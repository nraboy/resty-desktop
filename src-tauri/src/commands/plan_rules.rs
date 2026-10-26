//! The backup-plan rule engine: decides, for any path, whether `restic backup` would include
//! it — the same way restic does, without running restic.
//!
//! This is the foundation of the plan editor's file-tree view (which paths are included,
//! excluded, or *partially* included, and which rule decided). It must therefore match
//! restic's behaviour exactly, quirks included, so everything here is a literal port of
//! restic 0.17+'s `internal/filter` (`Match`, `ChildMatch`, `List`) and `internal/archiver`
//! (`RejectIfPresent`), on top of a port of Go's `path/filepath.Match`. The behaviour that is
//! easy to get wrong (verified against restic 0.19.1; see docs/restic.md):
//!
//! - A pattern matches any *contiguous run of path components*, not only the tail; only a
//!   leading `/` anchors it. `node_modules` therefore matches every path that has such a
//!   component, and an absolute pattern matches every path beneath it (prefix match).
//! - Patterns are evaluated in order and `!` cancels an earlier match
//!   ([`PatternList::evaluate`]). `--exclude` flags come first, then the contents of exclude
//!   files; case-insensitive (`--iexclude*`) patterns are a separate list that a case-sensitive
//!   `!` cannot cancel.
//! - restic evaluates every node it visits and does not descend into a rejected directory, so
//!   a node is included only if it *and every ancestor below its source root* pass
//!   ([`Engine::classify`]). Re-including something inside an excluded directory is
//!   impossible without re-including the directory first.
//! - A source path is exempt from the patterns itself (the root node), but not its contents.
//! - `exclude-if-present` rejects every item whose parent directory holds the marker file —
//!   except the marker itself; the directory stays.
//!
//! Pure Rust, no Tauri state and no I/O beyond the injected [`Probe`] (marker-file lookups).
//! Unix path semantics (`/`-separated, `\` escapes in patterns); Windows paths are a
//! follow-up. A rule is identified by a caller-chosen `id`, so a verdict can be traced back to
//! the line in the plan that caused it.

// Not called from the app yet — the plan editor's tree view (next phase) is its first user.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Mutex;

// ── Go's filepath.Match ─────────────────────────────────────────────────────

/// `filepath.ErrBadPattern`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadPattern;

/// Decodes the first rune of `s` like Go's `utf8.DecodeRuneInString`: invalid input is
/// `(U+FFFD, 1)`.
fn decode_rune(s: &[u8]) -> (char, usize) {
    if s.is_empty() {
        return ('\u{FFFD}', 0);
    }
    let len = match s[0] {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return ('\u{FFFD}', 1),
    };
    if s.len() < len {
        return ('\u{FFFD}', 1);
    }
    match std::str::from_utf8(&s[..len]) {
        Ok(st) => (st.chars().next().unwrap_or('\u{FFFD}'), len),
        Err(_) => ('\u{FFFD}', 1),
    }
}

/// Port of Go's `filepath.Match` (non-Windows). `name` is a single path component in
/// practice — restic only ever matches component by component — but the `/` rules of the
/// original are kept.
pub fn go_match(pattern: &str, name: &str) -> Result<bool, BadPattern> {
    let mut pattern = pattern.as_bytes();
    let mut name = name.as_bytes();
    'pattern: while !pattern.is_empty() {
        let (star, chunk, rest) = scan_chunk(pattern);
        pattern = rest;
        if star && chunk.is_empty() {
            // Trailing * matches the rest of the name unless it has a '/'.
            return Ok(!name.contains(&b'/'));
        }
        // Look for a match at the current position.
        let (t, ok) = match_chunk(chunk, name)?;
        // If this is the last chunk, make sure the name is exhausted; otherwise a false
        // result could hide a match that still works through the star.
        if ok && (t.is_empty() || !pattern.is_empty()) {
            name = t;
            continue;
        }
        if star {
            // Look for a match skipping i+1 bytes. Cannot skip '/'.
            let mut i = 0;
            while i < name.len() && name[i] != b'/' {
                let (t, ok) = match_chunk(chunk, &name[i + 1..])?;
                if ok {
                    // If this is the last chunk, make sure the name is exhausted.
                    if pattern.is_empty() && !t.is_empty() {
                        i += 1;
                        continue;
                    }
                    name = t;
                    continue 'pattern;
                }
                i += 1;
            }
        }
        return Ok(false);
    }
    Ok(name.is_empty())
}

fn scan_chunk(mut pattern: &[u8]) -> (bool, &[u8], &[u8]) {
    let mut star = false;
    while !pattern.is_empty() && pattern[0] == b'*' {
        pattern = &pattern[1..];
        star = true;
    }
    let mut inrange = false;
    let mut i = 0;
    while i < pattern.len() {
        match pattern[i] {
            b'\\' => {
                // An error for a trailing backslash is reported by match_chunk.
                if i + 1 < pattern.len() {
                    i += 1;
                }
            }
            b'[' => inrange = true,
            b']' => inrange = false,
            b'*' if !inrange => break,
            _ => {}
        }
        i += 1;
    }
    (star, &pattern[..i], &pattern[i..])
}

fn match_chunk<'a>(mut chunk: &[u8], mut s: &'a [u8]) -> Result<(&'a [u8], bool), BadPattern> {
    // `failed` records that the match has failed; the loop then keeps consuming the chunk
    // only to check that the pattern is well-formed.
    let mut failed = false;
    while !chunk.is_empty() {
        if !failed && s.is_empty() {
            failed = true;
        }
        match chunk[0] {
            b'[' => {
                let mut r = '\u{FFFD}';
                if !failed {
                    let (rr, n) = decode_rune(s);
                    r = rr;
                    s = &s[n..];
                }
                chunk = &chunk[1..];
                let mut negated = false;
                if !chunk.is_empty() && chunk[0] == b'^' {
                    negated = true;
                    chunk = &chunk[1..];
                }
                let mut matched = false;
                let mut nrange = 0;
                loop {
                    if !chunk.is_empty() && chunk[0] == b']' && nrange > 0 {
                        chunk = &chunk[1..];
                        break;
                    }
                    let (lo, rest) = get_esc(chunk)?;
                    chunk = rest;
                    let mut hi = lo;
                    // `get_esc` fails on an empty remainder, so chunk[0] is safe.
                    if chunk[0] == b'-' {
                        let (h, rest) = get_esc(&chunk[1..])?;
                        hi = h;
                        chunk = rest;
                    }
                    if lo <= r && r <= hi {
                        matched = true;
                    }
                    nrange += 1;
                }
                if matched == negated {
                    failed = true;
                }
            }
            b'?' => {
                if !failed {
                    if s[0] == b'/' {
                        failed = true;
                    }
                    let (_, n) = decode_rune(s);
                    s = &s[n..];
                }
                chunk = &chunk[1..];
            }
            _ => {
                if chunk[0] == b'\\' {
                    chunk = &chunk[1..];
                    if chunk.is_empty() {
                        return Err(BadPattern);
                    }
                }
                if !failed {
                    if chunk[0] != s[0] {
                        failed = true;
                    }
                    s = &s[1..];
                }
                chunk = &chunk[1..];
            }
        }
    }
    if failed {
        return Ok((&[], false));
    }
    Ok((s, true))
}

fn get_esc(mut chunk: &[u8]) -> Result<(char, &[u8]), BadPattern> {
    if chunk.is_empty() || chunk[0] == b'-' || chunk[0] == b']' {
        return Err(BadPattern);
    }
    if chunk[0] == b'\\' {
        chunk = &chunk[1..];
        if chunk.is_empty() {
            return Err(BadPattern);
        }
    }
    let (r, n) = decode_rune(chunk);
    if r == '\u{FFFD}' && n == 1 {
        return Err(BadPattern);
    }
    let rest = &chunk[n..];
    if rest.is_empty() {
        return Err(BadPattern);
    }
    Ok((r, rest))
}

// ── restic's filter: patterns ───────────────────────────────────────────────

/// One `/`-separated part of a compiled pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Part {
    /// The part's text; `**` is stored as the empty string, like restic does.
    text: String,
    /// No glob metacharacters (`\ [ ] * ?`): compared byte-for-byte.
    simple: bool,
}

/// A parsed `--exclude` pattern (restic's `filter.Pattern`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    original: String,
    parts: Vec<Part>,
    negated: bool,
}

/// Go's `filepath.Clean` for Unix paths (lexical: `.`, `..`, repeated and trailing slashes).
pub fn clean_path(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let rooted = path.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|c| *c != "..") {
                    out.pop();
                } else if !rooted {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    let joined = out.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// How paths are written, spelled and cleaned. The matching rules themselves are the same on
/// every platform — restic converts every path and pattern to `/`-separated components before it
/// matches — so the difference is confined to the edges: which spellings mean the same path, what
/// a root looks like, and whether a pattern can escape a glob character.
///
/// Windows (restic's `filepath` rules): `\` and `/` both separate, a path may start with a drive
/// (`C:`), and `\` is *not* an escape in a pattern — a literal `[` is written `[[]`. Everything is
/// kept internally in the `/` form (`C:/Users/me`), which the Windows APIs and restic both
/// accept; [`PathStyle::to_native`] gives back the `\` spelling for display and for the plan's
/// stored sources. UNC paths (`\\server\share`) are not supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    Unix,
    Windows,
}

impl PathStyle {
    /// The style of the platform this build runs on.
    pub const NATIVE: PathStyle = if cfg!(windows) { PathStyle::Windows } else { PathStyle::Unix };

    /// Go's `filepath.Clean`, then `ToSlash`: the canonical `/`-separated spelling.
    pub fn clean(self, path: &str) -> String {
        match self {
            PathStyle::Unix => clean_path(path),
            PathStyle::Windows => clean_windows(path),
        }
    }

    /// `name` inside `dir` (a root such as `/` or `C:/` already ends in a separator).
    pub fn join(self, dir: &str, name: &str) -> String {
        if dir.ends_with('/') {
            format!("{dir}{name}")
        } else {
            format!("{dir}/{name}")
        }
    }

    /// The directory containing a clean path; a root is its own parent.
    pub fn parent(self, path: &str) -> String {
        match path.rfind('/') {
            Some(0) => "/".to_string(),
            Some(i) if self == PathStyle::Windows && i == 2 && path.as_bytes()[1] == b':' => path[..3].to_string(),
            Some(i) => path[..i].to_string(),
            None => ".".to_string(),
        }
    }

    /// The spelling the OS uses: `\` separators on Windows.
    pub fn to_native(self, path: &str) -> String {
        match self {
            PathStyle::Unix => path.to_string(),
            PathStyle::Windows => path.replace('/', "\\"),
        }
    }

    /// Escapes glob metacharacters so a literal name can be used inside a pattern. Unix escapes
    /// with a backslash; on Windows a backslash is a separator, so a literal `[` is written `[[]`
    /// (`*` and `?` cannot occur in a Windows file name).
    pub fn escape_glob(self, s: &str) -> String {
        match self {
            PathStyle::Unix => {
                let mut out = String::with_capacity(s.len() + 2);
                for c in s.chars() {
                    if matches!(c, '\\' | '[' | ']' | '*' | '?') {
                        out.push('\\');
                    }
                    out.push(c);
                }
                out
            }
            PathStyle::Windows => s.replace('[', "[[]"),
        }
    }
}

impl Default for PathStyle {
    fn default() -> Self {
        PathStyle::NATIVE
    }
}

/// `filepath.Clean` + `ToSlash` for Windows paths: both separators, an optional drive (`C:`),
/// lexical `.`/`..`/repeated-separator handling. `C:\Users\me\` → `C:/Users/me`, `C:\` → `C:/`.
pub fn clean_windows(path: &str) -> String {
    let s = path.replace('\\', "/");
    let b = s.as_bytes();
    let (vol, rest) = if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        (&s[..2], &s[2..])
    } else {
        ("", &s[..])
    };
    if rest.is_empty() {
        return if vol.is_empty() { ".".to_string() } else { format!("{vol}.") };
    }
    let rooted = rest.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for comp in rest.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|c| *c != "..") {
                    out.pop();
                } else if !rooted {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    let joined = out.join("/");
    let body = match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    };
    format!("{vol}{body}")
}

/// Splits a clean path into components; an absolute path's first component is `"/"`.
fn split_path(p: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = p.split('/').collect();
    if parts[0].is_empty() {
        parts[0] = "/";
    }
    parts
}

impl Pattern {
    /// Parses `pattern` (restic's `preparePattern`), rejecting what restic's
    /// `ValidatePatterns` rejects: a part that is a malformed glob.
    pub fn parse(pattern: &str) -> Result<Pattern, BadPattern> {
        Pattern::parse_styled(pattern, PathStyle::Unix)
    }

    /// [`Pattern::parse`] with the path rules of `style` (Windows normalises `\` to `/` and
    /// understands drive letters before splitting the pattern into components).
    pub fn parse_styled(pattern: &str, style: PathStyle) -> Result<Pattern, BadPattern> {
        let original = pattern.to_string();
        let (negated, body) = match pattern.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, pattern),
        };
        let cleaned = style.clean(body);
        let parts: Vec<Part> = split_path(&cleaned)
            .into_iter()
            .map(|part| {
                let simple = !part.contains(['\\', '[', ']', '*', '?']);
                let text = if part == "**" { String::new() } else { part.to_string() };
                Part { text, simple }
            })
            .collect();
        // ValidatePatterns: every part must be a well-formed glob (matched against itself).
        for part in &parts {
            go_match(&part.text, &part.text)?;
        }
        Ok(Pattern { original, parts, negated })
    }

    pub fn is_negated(&self) -> bool {
        self.negated
    }

    pub fn original(&self) -> &str {
        &self.original
    }

    /// Anchored at the filesystem root (starts with `/`).
    fn is_absolute(&self) -> bool {
        self.parts.first().is_some_and(|p| p.text == "/")
    }

    /// A single literal component with no `/`, e.g. `node_modules`: matches iff some path
    /// component equals it. The index uses this to skip most patterns for most paths.
    fn simple_name(&self) -> Option<&str> {
        if !self.is_absolute() && self.parts.len() == 1 && self.parts[0].simple {
            Some(&self.parts[0].text)
        } else {
            None
        }
    }
}

/// Port of restic's `match`: does the pattern match `strs` (a path's components)?
///
/// restic expands each `**` into 0, 1, 2… single-component `*` wildcards and tries the result
/// at every offset; that is the same as matching the pattern against a contiguous run of
/// components where each `**` consumes any number of them, which is what this does — without
/// allocating. (`**` never consumes the leading `/` root component, and a pattern made only
/// of `**` cannot match an empty run.) One restic quirk is reproduced on purpose: its expansion
/// of a `**` stops at `len(path) - len(current pattern) + 1` components, where the current
/// pattern still counts every later `**` as a part, so a pattern with several `**` can fail to
/// match a path that a looser reading would accept. Tested against [`match_parts_reference`].
fn match_parts(parts: &[Part], strs: &[&str]) -> Result<bool, BadPattern> {
    if !parts.iter().any(|p| p.text.is_empty()) {
        return match_plain(parts, strs);
    }
    if strs.is_empty() {
        return Ok(false);
    }
    let (min_offset, max_offset) = if parts[0].text == "/" {
        (0, 0)
    } else {
        (usize::from(strs[0] == "/"), strs.len())
    };
    for offset in min_offset..=max_offset {
        if match_wild(parts, 0, strs, offset, offset, parts.len())? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn part_matches(part: &Part, s: &str) -> Result<bool, BadPattern> {
    if part.simple {
        Ok(part.text == s)
    } else {
        go_match(&part.text, s)
    }
}

/// Matches `parts[pi..]` against `strs[si..]`, where an empty part is `**`. `start` is where
/// the run began, so an all-`**` pattern can't match nothing; `cur_len` is the length restic's
/// expanded pattern has at this point (it bounds how much the next `**` may consume).
fn match_wild(
    parts: &[Part],
    pi: usize,
    strs: &[&str],
    si: usize,
    start: usize,
    cur_len: usize,
) -> Result<bool, BadPattern> {
    if pi == parts.len() {
        return Ok(si > start);
    }
    if parts[pi].text.is_empty() {
        // restic's bound; a path shorter than the pattern allows no expansion at all.
        let Some(budget) = (strs.len() + 1).checked_sub(cur_len) else { return Ok(false) };
        for i in 0..=budget {
            if i > 0 {
                // The i-th consumed component must exist and not be the '/' root marker, which
                // only a leading '/' in the pattern matches.
                let idx = si + i - 1;
                if idx >= strs.len() || strs[idx] == "/" {
                    return Ok(false);
                }
            }
            if match_wild(parts, pi + 1, strs, si + i, start, cur_len - 1 + i)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if si >= strs.len() || !part_matches(&parts[pi], strs[si])? {
        return Ok(false);
    }
    match_wild(parts, pi + 1, strs, si + 1, start, cur_len)
}

/// A pattern without `**`: slide it along the path's components.
fn match_plain(parts: &[Part], strs: &[&str]) -> Result<bool, BadPattern> {
    if parts.is_empty() && strs.is_empty() {
        return Ok(true);
    }
    // An empty pattern never matches a non-empty path.
    if parts.is_empty() {
        return Ok(false);
    }

    if parts.len() <= strs.len() {
        let mut min_offset = 0;
        let mut max_offset = strs.len() - parts.len();
        if parts[0].text == "/" {
            // Absolute patterns can only match at the very start.
            max_offset = 0;
        } else if strs[0] == "/" {
            // Skip the absolute-path marker for a relative pattern.
            min_offset = 1;
        }
        let mut offset = max_offset as isize;
        'outer: while offset >= min_offset as isize {
            let o = offset as usize;
            for i in (0..parts.len()).rev() {
                if !part_matches(&parts[i], strs[o + i])? {
                    offset -= 1;
                    continue 'outer;
                }
            }
            return Ok(true);
        }
    }
    Ok(false)
}

/// The literal port of restic's `match` (it expands `**` by allocating new patterns). Kept as
/// the reference the allocation-free [`match_parts`] is tested against.
#[cfg(test)]
fn match_parts_reference(parts: &[Part], strs: &[&str]) -> Result<bool, BadPattern> {
    if let Some(pos) = parts.iter().position(|p| p.text.is_empty()) {
        // Gradually expand '**' into separate '*' wildcards.
        // Go: `i <= len(strs)-len(parts)+1`, which is no iteration at all when negative.
        let Some(max) = (strs.len() + 1).checked_sub(parts.len()) else { return Ok(false) };
        for i in 0..=max {
            let mut new_parts: Vec<Part> = Vec::with_capacity(pos + i + parts.len());
            new_parts.extend_from_slice(&parts[..pos]);
            for _ in 0..i {
                new_parts.push(Part { text: "*".to_string(), simple: false });
            }
            new_parts.extend_from_slice(&parts[pos + 1..]);
            if match_parts_reference(&new_parts, strs)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }

    if parts.is_empty() && strs.is_empty() {
        return Ok(true);
    }
    // An empty pattern never matches a non-empty path.
    if parts.is_empty() {
        return Ok(false);
    }

    if parts.len() <= strs.len() {
        let mut min_offset = 0;
        let mut max_offset = strs.len() - parts.len();
        if parts[0].text == "/" {
            // Absolute patterns can only match at the very start.
            max_offset = 0;
        } else if strs[0] == "/" {
            // Skip the absolute-path marker for a relative pattern.
            min_offset = 1;
        }
        let mut offset = max_offset as isize;
        'outer: while offset >= min_offset as isize {
            let o = offset as usize;
            for i in (0..parts.len()).rev() {
                let ok = if parts[i].simple {
                    parts[i].text == strs[o + i]
                } else {
                    go_match(&parts[i].text, strs[o + i])?
                };
                if !ok {
                    offset -= 1;
                    continue 'outer;
                }
            }
            return Ok(true);
        }
    }
    Ok(false)
}

/// Port of restic's `childMatch`: can a path *below* `strs` still match? (Used for rule
/// analysis — "could anything under this folder be rejected?" — without touching the disk.)
fn child_match_parts(pattern: &Pattern, strs: &[&str]) -> Result<bool, BadPattern> {
    if pattern.parts[0].text != "/" {
        // A relative pattern can always be nested further down.
        return Ok(true);
    }
    let mut strs = strs;
    if let Some(pos) = pattern.parts.iter().position(|p| p.text.is_empty()) {
        if strs.len() >= pos {
            strs = &strs[..pos];
        }
    }
    let l = strs.len().min(pattern.parts.len());
    match_parts(&pattern.parts[..l], strs)
}

/// Restic's `filter.Match`/`ChildMatch` entry points: an empty pattern matches everything,
/// an empty path is an error. Mostly for tests.
pub fn pattern_matches(pattern: &str, path: &str) -> Result<bool, BadPattern> {
    pattern_matches_styled(pattern, path, PathStyle::Unix)
}

pub fn pattern_matches_styled(pattern: &str, path: &str, style: PathStyle) -> Result<bool, BadPattern> {
    if pattern.is_empty() {
        return Ok(true);
    }
    if path.is_empty() {
        return Err(BadPattern);
    }
    let p = Pattern::parse_styled(pattern, style)?;
    match_parts(&p.parts, &split_path(path))
}

pub fn pattern_child_matches(pattern: &str, path: &str) -> Result<bool, BadPattern> {
    pattern_child_matches_styled(pattern, path, PathStyle::Unix)
}

pub fn pattern_child_matches_styled(pattern: &str, path: &str, style: PathStyle) -> Result<bool, BadPattern> {
    if pattern.is_empty() {
        return Ok(true);
    }
    if path.is_empty() {
        return Err(BadPattern);
    }
    let p = Pattern::parse_styled(pattern, style)?;
    child_match_parts(&p, &split_path(path))
}

// ── restic's filter: ordered pattern lists ──────────────────────────────────

/// The outcome of evaluating one list against one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ListOutcome {
    /// Rejected by the list.
    pub matched: bool,
    /// Could a path below still match (for rule analysis)?
    pub child_may_match: bool,
    /// Index (into the list) of the last pattern that changed the outcome: the exclude that
    /// matched, or the `!` that cancelled it. `None` when no pattern ever matched.
    pub decided_by: Option<usize>,
}

/// An ordered list of patterns, evaluated like restic's `filter.List`, with a lookup index.
#[derive(Debug, Clone, Default)]
pub struct PatternList {
    patterns: Vec<Pattern>,
    /// Lower-case the path (and patterns) before matching: `--iexclude`.
    ignore_case: bool,
    /// Patterns that are a single literal name, keyed by that name → their positions.
    by_name: HashMap<String, Vec<usize>>,
    /// Positions of every other pattern (anchored, multi-part, or containing wildcards).
    other: Vec<usize>,
    /// Positions (ascending) of the non-negated single-name patterns. Even when they don't
    /// match, a relative pattern says "a child may still match", so skipping them in
    /// [`evaluate`](PatternList::evaluate) must not lose that.
    non_negated_names: Vec<usize>,
}

impl PatternList {
    pub fn new(patterns: Vec<Pattern>, ignore_case: bool) -> PatternList {
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        let mut other = Vec::new();
        let mut non_negated_names = Vec::new();
        for (i, p) in patterns.iter().enumerate() {
            match p.simple_name() {
                Some(name) => {
                    by_name.entry(name.to_string()).or_default().push(i);
                    if !p.negated {
                        non_negated_names.push(i);
                    }
                }
                None => other.push(i),
            }
        }
        PatternList { patterns, ignore_case, by_name, other, non_negated_names }
    }

    pub fn len(&self) -> usize {
        self.patterns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn pattern(&self, index: usize) -> &Pattern {
        &self.patterns[index]
    }

    /// Evaluates `path` (absolute, clean) against the list — restic's `List`/`ListWithChild`.
    /// Only patterns that can possibly match are visited, in their original order, which
    /// gives the same answer as visiting all of them: a skipped pattern is a literal name
    /// that no component equals, so it neither matches nor (if negated) cancels anything.
    pub fn evaluate(&self, path: &str) -> ListOutcome {
        if self.patterns.is_empty() {
            return ListOutcome::default();
        }
        let lowered;
        let path = if self.ignore_case {
            lowered = path.to_lowercase();
            lowered.as_str()
        } else {
            path
        };
        let strs = split_path(path);

        // Positions of the literal-name patterns some component equals. Usually none, in which
        // case the unmerged `other` list is walked directly with no allocation.
        let mut hits: Vec<usize> = Vec::new();
        if !self.by_name.is_empty() {
            for comp in strs.iter().skip(usize::from(strs[0] == "/")) {
                if let Some(h) = self.by_name.get(*comp) {
                    hits.extend_from_slice(h);
                }
            }
        }
        let merged: Vec<usize>;
        let candidates: &[usize] = if hits.is_empty() {
            &self.other
        } else {
            hits.extend_from_slice(&self.other);
            hits.sort_unstable();
            hits.dedup();
            merged = hits;
            &merged
        };

        let mut out = ListOutcome::default();
        let mut prev: Option<usize> = None;
        for &i in candidates {
            // Skipped non-negated names between the previous candidate and this one would each
            // have set "a child may match" (and a later `!` could clear it again).
            if self.skipped_name_between(prev, i) {
                out.child_may_match = true;
            }
            self.apply(&mut out, i, &strs);
            prev = Some(i);
        }
        if self.skipped_name_between(prev, usize::MAX) {
            out.child_may_match = true;
        }
        out
    }

    /// Is there a non-negated single-name pattern strictly between positions `a` and `b`?
    fn skipped_name_between(&self, a: Option<usize>, b: usize) -> bool {
        let lo = match a {
            Some(a) => self.non_negated_names.partition_point(|&p| p <= a),
            None => 0,
        };
        self.non_negated_names.get(lo).is_some_and(|&p| p < b)
    }

    /// Evaluates against *every* pattern with no index — the reference implementation the
    /// indexed path is tested against.
    pub fn evaluate_naive(&self, path: &str) -> ListOutcome {
        let lowered;
        let path = if self.ignore_case {
            lowered = path.to_lowercase();
            lowered.as_str()
        } else {
            path
        };
        let strs = split_path(path);
        self.fold(0..self.patterns.len(), &strs)
    }

    fn fold(&self, indices: impl Iterator<Item = usize>, strs: &[&str]) -> ListOutcome {
        let mut out = ListOutcome::default();
        for i in indices {
            self.apply(&mut out, i, strs);
        }
        out
    }

    /// One step of restic's `list` loop for the pattern at `i`.
    fn apply(&self, out: &mut ListOutcome, i: usize, strs: &[&str]) {
        let pat = &self.patterns[i];
        // restic logs a warning for a runtime pattern error and treats it as no match.
        let m = match_parts(&pat.parts, strs).unwrap_or(false);
        let c = child_match_parts(pat, strs).unwrap_or(false);
        if pat.negated {
            if out.matched && m {
                out.decided_by = Some(i);
            }
            out.matched = out.matched && !m;
            out.child_may_match = out.child_may_match && !m;
        } else {
            if m {
                out.decided_by = Some(i);
            }
            out.matched = out.matched || m;
            out.child_may_match = out.child_may_match || c;
        }
    }
}

// ── exclude-if-present markers ──────────────────────────────────────────────

/// One `--exclude-if-present name[:header]`; `--exclude-caches` is
/// `CACHEDIR.TAG:Signature: 8a477f597d28d172789f06886806bc55`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub name: String,
    /// If non-empty, the marker file must start with these bytes.
    pub header: String,
}

pub const CACHEDIR_TAG_SPEC: &str = "CACHEDIR.TAG:Signature: 8a477f597d28d172789f06886806bc55";

impl Marker {
    /// Parses a `name[:header]` spec; the name must not be empty.
    pub fn parse(spec: &str) -> Result<Marker, String> {
        if spec.is_empty() {
            return Err("name for exclusion tagfile is empty".to_string());
        }
        match spec.find(':') {
            Some(0) => Err("no name for exclusion tagfile provided".to_string()),
            Some(i) => Ok(Marker { name: spec[..i].to_string(), header: spec[i + 1..].to_string() }),
            None => Ok(Marker { name: spec.to_string(), header: String::new() }),
        }
    }
}

/// What the engine needs from the filesystem: whether a directory holds a marker file (with
/// the required header). Injected so the engine is testable without a disk.
pub trait Probe: Sync {
    /// Does `dir` contain a file called `marker.name` that starts with `marker.header`?
    fn has_marker(&self, dir: &str, marker: &Marker) -> bool;
}

/// The real filesystem, with a per-directory cache like restic's `rejectionCache`.
#[derive(Default)]
pub struct FsProbe {
    cache: Mutex<HashMap<(String, String), bool>>,
}

impl Probe for FsProbe {
    fn has_marker(&self, dir: &str, marker: &Marker) -> bool {
        let key = (dir.to_string(), marker.name.clone());
        if let Ok(cache) = self.cache.lock() {
            if let Some(v) = cache.get(&key) {
                return *v;
            }
        }
        let path = std::path::Path::new(dir).join(&marker.name);
        let present = match std::fs::symlink_metadata(&path) {
            Err(_) => false,
            Ok(_) if marker.header.is_empty() => true,
            Ok(_) => {
                use std::io::Read;
                let mut buf = vec![0u8; marker.header.len()];
                std::fs::File::open(&path)
                    .and_then(|mut f| f.read_exact(&mut buf))
                    .map(|_| buf == marker.header.as_bytes())
                    .unwrap_or(false)
            }
        };
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(key, present);
        }
        present
    }
}

// ── the engine ──────────────────────────────────────────────────────────────

/// One exclude/include pattern as the plan stores it, tagged with a caller-chosen id.
#[derive(Debug, Clone)]
pub struct RuleInput {
    pub id: u32,
    /// The pattern text, `!`-prefixed for a re-include.
    pub pattern: String,
    /// `--iexclude` semantics.
    pub ignore_case: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PlanRules {
    /// Absolute source paths (`restic backup <paths>`).
    pub sources: Vec<String>,
    /// Sources contributed by `--files-from` list files, already expanded (see `plan_listfiles`).
    /// The engine treats them exactly like `sources`; they are kept apart so edits never rewrite
    /// them (they belong to the list file) and the CLI export can name the file instead.
    pub list_sources: Vec<String>,
    /// The `--files-from` files themselves, for the CLI export only.
    pub files_from: Vec<String>,
    /// All patterns, in the order they reach restic (see the module docs for that order).
    pub rules: Vec<RuleInput>,
    /// `--exclude-if-present` specs.
    pub markers: Vec<String>,
    pub exclude_caches: bool,
    /// How the paths and patterns above are spelled (the platform's, unless a test says otherwise).
    pub style: PathStyle,
}

/// Why a node is excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// A pattern rule matched this node (or an ancestor, see `Verdict::at`).
    Pattern { rule_id: u32 },
    /// The node's parent directory holds an exclude-if-present marker.
    Marker { spec: String },
    /// Not at or below any source path.
    OutsideSources,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub included: bool,
    pub reason: Option<Reason>,
    /// The (ancestor or own) path the reason applies to, when it isn't the queried path.
    pub at: Option<String>,
    /// For an included node: the `!` rule that last re-included it, if it was excluded
    /// before that.
    pub reincluded_by: Option<u32>,
}

impl Verdict {
    fn included() -> Verdict {
        Verdict { included: true, reason: None, at: None, reincluded_by: None }
    }
}

pub struct Engine {
    style: PathStyle,
    sources: Vec<String>,
    /// Case-sensitive patterns (`--exclude`, `--exclude-file`), with the rule id of each.
    cs: PatternList,
    cs_ids: Vec<u32>,
    /// Case-insensitive patterns (`--iexclude`, `--iexclude-file`).
    ci: PatternList,
    ci_ids: Vec<u32>,
    markers: Vec<(Marker, String)>,
}

impl Engine {
    /// Compiles `rules`. A malformed pattern is an error naming it (restic refuses to run).
    pub fn new(rules: &PlanRules) -> Result<Engine, String> {
        let mut cs = Vec::new();
        let mut cs_ids = Vec::new();
        let mut ci = Vec::new();
        let mut ci_ids = Vec::new();
        for r in &rules.rules {
            if r.pattern.is_empty() {
                continue; // restic skips empty patterns
            }
            let text = if r.ignore_case { r.pattern.to_lowercase() } else { r.pattern.clone() };
            let pat = Pattern::parse_styled(&text, rules.style)
                .map_err(|_| format!("invalid pattern: {}", r.pattern))?;
            if r.ignore_case {
                ci.push(pat);
                ci_ids.push(r.id);
            } else {
                cs.push(pat);
                cs_ids.push(r.id);
            }
        }
        let mut specs: Vec<String> = rules.markers.clone();
        if rules.exclude_caches {
            specs.push(CACHEDIR_TAG_SPEC.to_string());
        }
        let markers = specs
            .into_iter()
            .map(|s| Marker::parse(&s).map(|m| (m, s)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Engine {
            style: rules.style,
            sources: {
                let mut seen = std::collections::HashSet::new();
                rules
                    .sources
                    .iter()
                    .chain(&rules.list_sources)
                    .map(|s| rules.style.clean(s))
                    .filter(|c| seen.insert(c.clone()))
                    .collect()
            },
            cs: PatternList::new(cs, false),
            cs_ids,
            ci: PatternList::new(ci, true),
            ci_ids,
            markers,
        })
    }

    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// How this engine's paths are spelled (see [`PathStyle`]).
    pub fn style(&self) -> PathStyle {
        self.style
    }

    /// Are there any patterns or markers at all? With none, every node under a source is
    /// included and no scan needs to touch the disk.
    pub fn has_rules(&self) -> bool {
        !self.cs.is_empty() || !self.ci.is_empty() || !self.markers.is_empty()
    }

    /// Would restic reject *this node by itself* — by pattern or marker — ignoring its
    /// ancestors? `None` means it passes. (The source root is exempt: callers skip it.)
    pub fn judge_node(&self, path: &str, probe: &dyn Probe) -> Option<Reason> {
        let a = self.cs.evaluate(path);
        if a.matched {
            if let Some(i) = a.decided_by {
                return Some(Reason::Pattern { rule_id: self.cs_ids[i] });
            }
        }
        let b = self.ci.evaluate(path);
        if b.matched {
            if let Some(i) = b.decided_by {
                return Some(Reason::Pattern { rule_id: self.ci_ids[i] });
            }
        }
        let base = path.rsplit('/').next().unwrap_or(path);
        let dir = self.style.parent(path);
        for (marker, spec) in &self.markers {
            if base != marker.name && probe.has_marker(&dir, marker) {
                return Some(Reason::Marker { spec: spec.clone() });
            }
        }
        None
    }

    /// The `!` rule (if any) that last re-included `path`, for provenance.
    fn reincluded_by(&self, path: &str) -> Option<u32> {
        for (list, ids) in [(&self.cs, &self.cs_ids), (&self.ci, &self.ci_ids)] {
            let out = list.evaluate(path);
            if !out.matched {
                if let Some(i) = out.decided_by {
                    if list.pattern(i).is_negated() {
                        return Some(ids[i]);
                    }
                }
            }
        }
        None
    }

    /// Is `path` backed up? It must be at or below a source and, for every directory between
    /// that source and `path` (and `path` itself), pass [`judge_node`](Engine::judge_node) —
    /// restic does not descend into a rejected directory. A node under several sources is
    /// included if any of them includes it.
    pub fn classify(&self, path: &str, probe: &dyn Probe) -> Verdict {
        let path = self.style.clean(path);
        let mut first_rejection: Option<Verdict> = None;
        let mut under_any = false;
        for src in &self.sources {
            let Some(rel) = strip_source(&path, src) else { continue };
            under_any = true;
            if rel.is_empty() {
                return Verdict::included(); // the source root itself is exempt
            }
            let mut current = src.clone();
            let mut rejected: Option<Verdict> = None;
            for comp in rel.split('/') {
                current = self.style.join(&current, comp);
                if let Some(reason) = self.judge_node(&current, probe) {
                    rejected = Some(Verdict {
                        included: false,
                        reason: Some(reason),
                        at: (current != path).then(|| current.clone()),
                        reincluded_by: None,
                    });
                    break;
                }
            }
            match rejected {
                None => {
                    let mut v = Verdict::included();
                    v.reincluded_by = self.reincluded_by(&path);
                    return v;
                }
                Some(v) => {
                    first_rejection.get_or_insert(v);
                }
            }
        }
        if under_any {
            first_rejection.unwrap_or_else(Verdict::included)
        } else {
            Verdict { included: false, reason: Some(Reason::OutsideSources), at: None, reincluded_by: None }
        }
    }
}

/// The root a clean canonical path starts at — `/`, or a drive such as `C:/` — or `None` for a
/// relative path.
pub fn root_of(path: &str) -> Option<&str> {
    let b = path.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/' {
        return Some(&path[..3]);
    }
    path.starts_with('/').then(|| &path[..1])
}

/// Is this canonical path a root (`/`, `C:/`)?
pub fn is_root(path: &str) -> bool {
    root_of(path) == Some(path)
}

/// `path` relative to `src` when it is `src` or below it (`""` for `src` itself). A root
/// (`/`, `C:/`) already ends in a separator.
pub(crate) fn strip_source<'a>(path: &'a str, src: &str) -> Option<&'a str> {
    if path == src {
        return Some("");
    }
    if src.ends_with('/') {
        return path.strip_prefix(src);
    }
    path.strip_prefix(src).and_then(|rest| rest.strip_prefix('/'))
}

#[cfg(test)]
mod test_tables;
#[cfg(test)]
mod tests;
