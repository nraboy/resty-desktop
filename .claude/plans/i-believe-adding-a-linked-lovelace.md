# Add unit-test coverage for restic-adjacent pure logic

## Context

The prior change in this session wired up a narrow linter (react-hooks + clippy) and, in the
process, `cargo clippy --fix` rewrote several bits of Rust. During review we found that some of
the touched code has **zero automated test coverage** — notably `set_restic_path`'s path
validation and all of `cache_warmer.rs` — and that the two stats/diff parsers I edited (the
`rfind` and `strip_prefix` changes) are also untested. Those changes were verified only by manual
tracing plus two throwaway proof scripts, not by anything that runs in CI.

This plan adds real, permanent coverage for the pure logic that surrounds our restic subprocess
calls. The external `restic` binary dependency only blocks testing the *subprocess invocation
itself* — the surrounding parsing/validation/branching is separable and directly testable. The
approach mirrors the existing precedent in the codebase: `snapshot.rs` already extracts pure
helpers (`build_retention_args` at `snapshot.rs:864`, `validate_snapshot_id` at `snapshot.rs:153`)
out of its `#[tauri::command]`s specifically so they can be unit-tested in a `#[cfg(test)] mod
tests` block. We replicate that pattern for the untested parsers/validators.

**Intended outcome:** the specific lines clippy rewrote this session (stats-line finding, diff
line parsing) plus the previously-untested `set_restic_path` validation and `is_remote` helper are
locked in by unit tests, so a future refactor that breaks them fails CI instead of shipping.

## What is / isn't testable (scope boundary)

- **Testable now (this plan):** pure functions that take strings/values and return
  parsed/validated results — no `tauri::State`, no `AppHandle`, no live restic. These are extracted
  (where not already separate) and tested directly.
- **Deliberately out of scope:** `cache_warmer::index_next` / `trigger_sweep` /
  `refresh_all_snapshots`, and the `#[tauri::command]` bodies themselves. They're bound to live
  `tauri::State`/`AppHandle` and shelling out — exercising them needs a full mock-Tauri harness,
  a much larger investment than this warrants. Note this explicitly in the plan so it isn't
  mistaken for an oversight.

## Changes

The unifying pattern (from `build_retention_args`): pull the pure logic out of the async command
into a private `fn` that takes already-captured `&str`/values, have the command call it, then unit
test the `fn` with fixture strings. The command keeps doing the restic call + `RepoLocks` guard +
DB write; only the parsing/validation moves.

### 1. Stats-line parsing — `repo.rs` + `snapshot.rs`

Both `fetch_and_cache_stats` (`repo.rs:174`) and `get_snapshot_stats` (`snapshot.rs:238`) do the
same thing: take restic `stats --json` stdout, find the **last non-blank line** (the line I changed
`.filter(..).last()` → `.rfind(..)`), parse it as JSON, pull fields with `.as_u64().unwrap_or(0)`.

- Extract a pure helper in `repo.rs`, e.g.
  `fn parse_stats_json(stdout: &str) -> Result<ResticStats, String>` — does the `rfind` +
  `serde_json::from_str` + field extraction, returns the existing `ResticStats` struct
  (`repo.rs:10`). `fetch_and_cache_stats` calls it, then does its `db.set_stats(...)`.
- `get_snapshot_stats` returns a different, smaller struct (`SnapshotStats` — `total_size`,
  `total_file_count`). Give it its own tiny helper (or a shared last-non-blank-line helper +
  per-caller field extraction). Prefer a shared
  `fn last_nonblank_line(s: &str) -> Option<&str>` so the exact `rfind` logic is tested once and
  both callers reuse it.
- **Tests** (new `#[cfg(test)] mod tests` in `repo.rs`; existing one in `snapshot.rs`):
  - well-formed single-line JSON → correct fields
  - **multi-line stdout with trailing blank lines and mid-output noise** → picks the last real
    JSON line (this is the exact behavior the `rfind` change preserves — the regression guard)
  - missing fields → `unwrap_or(0)` defaults, not an error
  - empty / all-blank stdout → the "No output from restic stats" `Err`
  - malformed JSON on the last line → parse `Err`

### 2. Diff text parsing — `snapshot.rs`

`diff_snapshots` (`snapshot.rs:187`) parses `restic diff` plain-text: lines prefixed `+  `, `-  `,
`M  `, `T  ` (the `strip_prefix` code I edited), counting added/removed/modified, building
`DiffEntry`s, capping at `DIFF_ENTRY_LIMIT` (500, `snapshot.rs:184`) with a `truncated` flag.

- Extract `fn parse_diff_output(stdout: &str) -> DiffResult` containing the whole
  `for line in stdout.lines() { ... }` loop + totals + truncation. `diff_snapshots` keeps the
  restic call + `RepoLocks` guard, then calls the helper on captured stdout.
- **Tests** (in `snapshot.rs`'s existing `mod tests`):
  - each prefix type (`+`/`-`/`M`/`T`) maps to the right `change` string and increments the right
    counter; `T` and `M` both → `"modified"`
  - non-matching / unprefixed lines are skipped (no panic, no miscount)
  - `path` is trimmed (matches `snapshot.rs:228`)
  - **truncation:** feed > 500 matching lines → `entries.len() == 500`, `truncated == true`, and
    the totals still reflect the *full* count (this is the `total as usize > entries.len()` logic
    at `snapshot.rs:233`)
  - empty input → all zeros, `truncated == false`

### 3. `set_restic_path` validation — `repo.rs`

`set_restic_path` (`repo.rs:274`) — the empty-check + "looks absolute → must be an existing file"
branch I edited (the collapsed `if`). The DB write (`db.set_setting`) is the only non-pure part.

- Extract `fn validate_restic_path(trimmed: &str) -> Result<(), String>` (empty check + the
  absolute-path-exists check). Command trims, calls the validator, then `db.set_setting`.
- **Tests** (new `mod tests` in `repo.rs`, or the one added in §1):
  - empty / whitespace-only → `Err` "must not be empty"
  - bare `"restic"` (no separator) → `Ok` (not treated as a path, never stat'd)
  - absolute path to a file that exists (use the current test binary's own path via
    `std::env::current_exe()`, which is guaranteed to exist) → `Ok`
  - absolute path to a nonexistent file (e.g. `/nonexistent/xyz/restic`) → `Err` "No file found"
  - covers all three separator forms (`/`, `\`, `:\`) at least at the branch level

### 4. `cache_warmer::is_remote` — `cache_warmer.rs`

`is_remote` (`cache_warmer.rs:18`) is already pure (`REMOTE_PREFIXES` prefix check) but untested,
and it gates remote-repo skipping in three places.

- **Tests** (new `#[cfg(test)] mod tests` in `cache_warmer.rs`): each prefix in `REMOTE_PREFIXES`
  (`s3:`, `sftp:`, `rest:`, `azure:`, `gs:`, `b2:`, `rclone:`) → `true`; a local absolute path and
  a bare relative path → `false`. Guards the list against accidental edits.

### 5. `describe_cron` whitespace case — `schedule.rs`

`describe_cron` (`schedule.rs:27`) is already tested, but the `.trim()` removal from this session's
clippy fix isn't pinned by a test. Add one assertion to the existing `test_describe_cron_*` suite:
a whitespace-padded expression (`"  0 0 * * *  "`) → same result as the unpadded form. Locks in the
`split_whitespace()`-handles-padding fact permanently (replacing my throwaway proof script).

## Files

- `src-tauri/src/commands/repo.rs` — extract `validate_restic_path`, `parse_stats_json` (+ shared
  `last_nonblank_line`); **new** `#[cfg(test)] mod tests`
- `src-tauri/src/commands/snapshot.rs` — extract `parse_diff_output`; reuse `last_nonblank_line`
  for `get_snapshot_stats`; add tests to existing `mod tests`
- `src-tauri/src/cache_warmer.rs` — **new** `#[cfg(test)] mod tests` for `is_remote`
- `src-tauri/src/commands/schedule.rs` — one added assertion in existing cron tests

No production behavior changes — this is pure extraction (same code, moved) + new tests. The
extracted helpers must be byte-for-byte behavior-preserving so the `clippy --fix` results stay
verified rather than re-risked.

## Verification

1. `npm run test:rust` — all new tests pass alongside the existing 84.
2. `npm run typecheck && npm run lint:all` — still clean (extraction shouldn't introduce clippy
   warnings; watch for a new `too_many_arguments` or dead-code warning on the helpers).
3. `npm run test:all` — full suite (Rust + the 53 Vitest) green.
4. Sanity-check the extraction is truly behavior-preserving: confirm each new helper is called from
   the original command site and the command's observable output path is unchanged (guard +
   restic call + DB write still in the same order).
```bash
npm run test:rust
npm run lint:all
npm run test:all
```
