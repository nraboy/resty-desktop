//! Backend registry for remote repository credentials.
//!
//! Pure, `cfg`-free, no Tauri state and no I/O — see CLAUDE.md's "Backend credentials"
//! section. `detect_kind` is the single source of truth for a repo's backend kind;
//! it is never persisted (see that section's rationale), so a path edit that changes
//! the backend is caught by re-validating credentials against the newly-detected kind
//! rather than by comparing against a stored value that could drift out of sync.

/// Which restic backend a repository path targets. Derived from the path via
/// `detect_kind` — never stored, so it can never disagree with the path it was
/// derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Local,
    S3,
    B2,
    /// restic's REST backend (`rest:`). Unlike S3/B2 this accepts an arbitrary
    /// key/value list like `Other` — see `validate_credentials` — because every
    /// `rest:` repo predating this variant was `Other` and may already carry one.
    Rest,
    /// Any other remote prefix (sftp:, azure:, gs:, rclone:, …) or an
    /// unrecognized string. Accepts an arbitrary key/value credential list with
    /// no validation beyond the shared PATH/RESTIC_* denylist.
    Other,
}

/// One credential this backend kind accepts, and whether restic requires it.
pub struct CredentialSpec {
    pub env_key: &'static str,
    pub required: bool,
}

const S3_SPECS: &[CredentialSpec] = &[
    CredentialSpec { env_key: "AWS_ACCESS_KEY_ID", required: true },
    CredentialSpec { env_key: "AWS_SECRET_ACCESS_KEY", required: true },
    CredentialSpec { env_key: "AWS_SESSION_TOKEN", required: false },
    CredentialSpec { env_key: "AWS_DEFAULT_REGION", required: false },
];

const B2_SPECS: &[CredentialSpec] = &[
    CredentialSpec { env_key: "B2_ACCOUNT_ID", required: true },
    CredentialSpec { env_key: "B2_ACCOUNT_KEY", required: true },
];

/// Both optional: an unauthenticated REST server needs neither, and a repo whose
/// credentials are still inline in the URL needs neither (restic's ApplyEnvironment
/// ignores these entirely when the URL carries userinfo).
const REST_SPECS: &[CredentialSpec] = &[
    CredentialSpec { env_key: "RESTIC_REST_USERNAME", required: false },
    CredentialSpec { env_key: "RESTIC_REST_PASSWORD", required: false },
];

const EMPTY_SPECS: &[CredentialSpec] = &[];

/// The credential fields restic recognizes for `kind`. `Local` and `Other` have no
/// fixed set — `Other` accepts an arbitrary key/value list (see `validate_credentials`).
pub fn credential_specs(kind: BackendKind) -> &'static [CredentialSpec] {
    match kind {
        BackendKind::S3 => S3_SPECS,
        BackendKind::B2 => B2_SPECS,
        BackendKind::Rest => REST_SPECS,
        BackendKind::Local | BackendKind::Other => EMPTY_SPECS,
    }
}

/// Same prefix list `isRemoteRepo` uses on the frontend (`src/lib/types.ts`) — kept
/// in sync manually since no test spans the two languages. `"rest:"` stays listed
/// here even though `detect_kind` below now matches it in its own dedicated arm
/// first: this list is also what backs `isRemoteRepo`-equivalent "is this a remote
/// repo" checks (remote_auto_refresh gating, the cache warmer, SnapshotsPage's
/// background refresh), not just `detect_kind`'s dispatch. Removing it would silently
/// stop treating REST repos as remote for those purposes.
const REMOTE_PREFIXES: &[&str] = &["s3:", "sftp:", "rest:", "azure:", "gs:", "b2:", "rclone:"];

/// Derives a repository's backend kind from its path. Total — every path resolves to
/// some kind, so this never fails and there is nothing to migrate for existing repos.
pub fn detect_kind(path: &str) -> BackendKind {
    if path.starts_with("s3:") {
        BackendKind::S3
    } else if path.starts_with("b2:") {
        BackendKind::B2
    } else if path.starts_with("rest:") {
        BackendKind::Rest
    } else if REMOTE_PREFIXES.iter().any(|p| path.starts_with(p)) {
        BackendKind::Other
    } else {
        BackendKind::Local
    }
}

/// True when `rest-server --append-only` means something for this path: a `rest:http(s)://`
/// URL (it is a rest-server option; no other backend has it). Mirrors the frontend's
/// `supportsAppendOnly` (`src/lib/backends.ts`), which is what keeps the flag off every other
/// path in the add/edit forms — this is the check for input that doesn't come through them.
pub fn supports_append_only(path: &str) -> bool {
    // `rest:` itself is case-sensitive (restic rejects `REST:`); the scheme after it is not.
    path.trim()
        .strip_prefix("rest:")
        .is_some_and(|rest| {
            let rest = rest.to_ascii_lowercase();
            rest.starts_with("http://") || rest.starts_with("https://")
        })
}

/// The only `RESTIC_*` vars a stored credential may set. Unlike RESTIC_REPOSITORY /
/// RESTIC_PASSWORD / RESTIC_FROM_* / RESTIC_COMPRESSION, the app never sets these
/// itself, so there is no collision for a stored value to win and nothing to
/// redirect — they only select HTTP basic auth for the REST backend. Kept as an
/// explicit allowlist rather than a `RESTIC_REST_` prefix match so a future
/// `RESTIC_REST_*` var restic may add doesn't become settable by accident.
const ALLOWED_RESTIC_KEYS: &[&str] = &["RESTIC_REST_USERNAME", "RESTIC_REST_PASSWORD"];

/// Env var names the app controls itself and a stored credential must never set:
/// `PATH` (see `NoConsole::augment_path`), every `RESTIC_*` var except the two
/// REST auth vars in `ALLOWED_RESTIC_KEYS` (repository, password, compression, …
/// all stay reserved), and the dynamic-loader families `LD_*` / `DYLD_*`
/// (`LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`, … — code-execution primitives on the restic
/// child; no restic backend uses them). A prefix match, so a future loader var is
/// covered too. Matching is case-insensitive because Windows env names are: a
/// credential named `restic_repository` or `Path` would otherwise slip past and override
/// the very variable this guards. Other arbitrary keys (`HTTPS_PROXY`, …) stay allowed for
/// `Other`/`Rest` — see docs/decisions.md. Shared by `validate_credentials` — which rejects
/// such a key at entry, the earlier and louder guard — and `repo::apply_backend_env`, which
/// skips it at apply time so the guarantee holds even for a credential that reached
/// the DB some other way (e.g. a hand-edited import bundle).
pub fn is_reserved_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    key == "PATH"
        || key.starts_with("LD_")
        || key.starts_with("DYLD_")
        || (key.starts_with("RESTIC_") && !ALLOWED_RESTIC_KEYS.contains(&key.as_str()))
}

/// Validates a proposed credential set for `kind`. An empty set is always valid —
/// that's the "use restic's own credential chain" ambient mode (see CLAUDE.md) — so
/// this only enforces shape once the user has actually entered something.
///
/// Every kind rejects a reserved (`PATH`/`RESTIC_*`) key outright — see
/// `is_reserved_key` and `repo::apply_backend_env` for why this is defense in depth
/// rather than the only guard — and a key listed more than once, which would
/// otherwise resolve inconsistently (`cache::encode_credentials` keeps the last
/// duplicate via its `HashMap`, `repo::merge_credentials` keeps the first via its
/// `Vec::find`).
pub fn validate_credentials(kind: BackendKind, creds: &[(String, String)]) -> Result<(), String> {
    if creds.is_empty() {
        return Ok(());
    }

    for (key, _) in creds {
        if is_reserved_key(key) {
            return Err(format!(
                "'{key}' is a reserved variable and cannot be set as a backend credential"
            ));
        }
    }

    for (i, (key, _)) in creds.iter().enumerate() {
        if creds[..i].iter().any(|(prev, _)| prev == key) {
            return Err(format!("'{key}' is listed more than once"));
        }
    }

    let specs = credential_specs(kind);
    // Rest is deliberately excluded from this unknown-key check, unlike S3/B2: a
    // rest: repo predating BackendKind::Rest was Other, which allows arbitrary keys,
    // and may already have one stored (e.g. HTTPS_PROXY). Tightening this would break
    // such a repo's validation on its next edit/test/import — see CLAUDE.md's
    // Intentional Designs and `validate_credentials_allows_arbitrary_keys_for_rest`.
    if matches!(kind, BackendKind::S3 | BackendKind::B2) {
        for (key, _) in creds {
            if !specs.iter().any(|s| s.env_key == *key) {
                return Err(format!("'{key}' is not a recognized credential for this backend"));
            }
        }
    }

    for spec in specs {
        if spec.required {
            let has_value = creds
                .iter()
                .any(|(k, v)| k == spec.env_key && !v.trim().is_empty());
            if !has_value {
                return Err(format!("Missing required credential: {}", spec.env_key));
            }
        }
    }

    Ok(())
}

/// Returned (as the start of the error text) when a connection test gives up waiting on
/// a server restic is still retrying — see `repo::run_restic_with_timeout`.
pub const CONNECTION_TIMEOUT_MARKER: &str = "Timed out waiting for the server";

/// A short, human explanation for a failed restic call against a `rest:` repository, or
/// `None` when the failure isn't one we recognize (or the repo isn't REST). Pure — it only
/// reads restic's stderr, which carries the server's HTTP status in every case below:
/// `Stat(<config/>) failed: unexpected HTTP response (401): 401 Unauthorized`.
///
/// What each status means for rest-server (verified against rest-server's source and a live
/// instance): a read such as the connection test (`restic snapshots`) can only see
/// **401** (bad/missing credentials, *or* a `--private-repos` path outside `/<username>/` —
/// the server doesn't distinguish them) and **404** (unknown path or an uninitialised
/// repo). **403** only comes from write-type calls: deleting or modifying existing data
/// under `--append-only` (forget, prune, tag), or re-creating config/blobs that exist.
/// **507** is `--max-size` / disk-full on a write.
///
/// `insecure_tls` suppresses the certificate hint: with verification already off, an x509
/// error isn't something the checkbox can fix.
pub fn rest_error_hint(path: &str, insecure_tls: bool, stderr: &str) -> Option<String> {
    if !path.starts_with("rest:") {
        return None;
    }
    let e = stderr.to_ascii_lowercase();
    let over_http = path.to_ascii_lowercase().starts_with("rest:http://");

    if e.contains("server gave http response to https client") {
        return Some(
            "The server answered in plain HTTP. Change the URL to start with rest:http:// \
             (or start rest-server with --tls)."
                .to_string(),
        );
    }
    if e.contains("x509:") || e.contains("failed to verify certificate") {
        if insecure_tls {
            return None;
        }
        return Some(
            "Couldn't verify the server's TLS certificate (typical for a self-signed certificate). \
             Recommended: add the server's certificate, or the CA that signed it, to your \
             operating system's certificate store. Alternatively, tick \"Skip TLS certificate \
             verification (--insecure-tls)\" for this repository."
                .to_string(),
        );
    }
    if has_http_status(&e, 401) {
        return Some(
            "Authentication failed (401). Check the username and password. If rest-server runs \
             with --private-repos, the repository path must start with your username, e.g. \
             rest:https://host:8000/<username>/."
                .to_string(),
        );
    }
    if has_http_status(&e, 507) {
        return Some(
            "The server is out of storage space or has reached its size limit (507) — check the \
             disk or rest-server's --max-size."
                .to_string(),
        );
    }
    if has_http_status(&e, 403) {
        // The hint is appended to any failed `rest:` call, not only deletes, so it names both
        // usual causes rather than assuming append-only.
        return Some(
            "The server refused the request (403 Forbidden). On a rest-server this usually means \
             it was started with --append-only, which does not allow deleting or modifying \
             existing data (forget, prune and tag fail while backups and restores still work), \
             or that an access rule or proxy in front of the server is blocking the request."
                .to_string(),
        );
    }
    if has_http_status(&e, 404) || e.contains("repository does not exist") {
        return Some(
            "Repository not found (404). Check the path in the URL — with --private-repos it starts \
             with your username — or initialise a new repository at this URL first."
                .to_string(),
        );
    }
    if over_http && has_http_status(&e, 400) {
        return Some(
            "The server rejected a plain-HTTP request (400). If rest-server runs with --tls, use \
             rest:https:// instead."
                .to_string(),
        );
    }
    if e.contains(&CONNECTION_TIMEOUT_MARKER.to_ascii_lowercase())
        || e.contains("connection refused")
        || e.contains("no such host")
        || e.contains("i/o timeout")
        || e.contains("no route to host")
        || e.contains("network is unreachable")
        || e.contains("retrying after")
    {
        return Some(
            "Couldn't reach the server. Check the host name, port and http/https scheme, and that \
             rest-server is running and reachable from this machine."
                .to_string(),
        );
    }
    None
}

/// True when the (lower-cased) stderr reports `code` in restic's own
/// `unexpected HTTP response (NNN)` shape. Matched on that phrase rather than a bare `(404)`:
/// a failed backup's stderr also carries file names and OS messages, and a file called
/// `report (404).pdf` must not be read as the server's answer.
fn has_http_status(e: &str, code: u16) -> bool {
    e.contains(&format!("unexpected http response ({code})"))
}

/// `err` with a `rest_error_hint` appended on its own paragraph when one applies, else
/// `err` unchanged. Appended (never prepended or replacing) so callers that match restic's
/// own text — the retry-on-"already locked" checks — keep working.
pub fn append_rest_hint(path: &str, insecure_tls: bool, err: String) -> String {
    match rest_error_hint(path, insecure_tls, &err) {
        Some(hint) => format!("{}\n\nHint: {hint}", err.trim_end()),
        None => err,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn supports_append_only_only_for_rest_http_urls() {
        assert!(supports_append_only("rest:http://nas:8000/repo"));
        assert!(supports_append_only("rest:https://nas:8000/repo"));
        // The scheme after `rest:` is case-insensitive, the `rest:` prefix is not (matches the
        // frontend and `verifies_tls`), and surrounding whitespace is ignored like the forms do.
        assert!(supports_append_only("rest:HTTPS://nas/r"));
        assert!(supports_append_only("  rest:http://nas/r  "));
        assert!(!supports_append_only("REST:https://nas/r"));
        // No other backend has the option — nor does a unix-socket rest URL.
        for path in ["/backups/local", "sftp:u@h:/r", "s3:s3.amazonaws.com/b", "rclone:remote:p", "rest:http+unix:///tmp/s:/r", "rest:", ""] {
            assert!(!supports_append_only(path), "{path}");
        }
    }

    #[test]
    fn detect_kind_matches_known_prefixes() {
        assert_eq!(detect_kind("s3:s3.amazonaws.com/bucket"), BackendKind::S3);
        assert_eq!(detect_kind("b2:my-bucket:restic"), BackendKind::B2);
        assert_eq!(detect_kind("sftp:user@host:/path"), BackendKind::Other);
        assert_eq!(detect_kind("rest:http://localhost:8000/"), BackendKind::Rest);
        assert_eq!(detect_kind("azure:container:/"), BackendKind::Other);
        assert_eq!(detect_kind("gs:bucket:/"), BackendKind::Other);
        assert_eq!(detect_kind("rclone:remote:path"), BackendKind::Other);
    }

    #[test]
    fn detect_kind_rest_is_case_sensitive_like_the_others() {
        assert_eq!(detect_kind("REST:https://host/"), BackendKind::Local);
    }

    #[test]
    fn detect_kind_rest_still_counts_as_a_remote_prefix() {
        // Guards against removing "rest:" from REMOTE_PREFIXES now that the dedicated
        // arm above shadows it for detect_kind's own dispatch — that list is also what
        // backs remote_auto_refresh gating (isRemoteRepo on the frontend), which has no
        // other way to know a rest: repo is remote.
        assert!(REMOTE_PREFIXES.contains(&"rest:"));
    }

    #[test]
    fn credential_specs_for_rest_are_both_optional() {
        let specs = credential_specs(BackendKind::Rest);
        assert_eq!(specs.len(), 2);
        assert!(specs.iter().all(|s| !s.required));
        assert!(specs.iter().any(|s| s.env_key == "RESTIC_REST_USERNAME"));
        assert!(specs.iter().any(|s| s.env_key == "RESTIC_REST_PASSWORD"));
    }

    #[test]
    fn detect_kind_defaults_to_local() {
        assert_eq!(detect_kind("/home/user/backups"), BackendKind::Local);
        assert_eq!(detect_kind("C:\\backups"), BackendKind::Local);
    }

    #[test]
    fn detect_kind_is_case_sensitive() {
        // Matches isRemoteRepo's deliberate case-sensitivity (types.test.ts).
        assert_eq!(detect_kind("S3:bucket"), BackendKind::Local);
        assert_eq!(detect_kind("B2:bucket:path"), BackendKind::Local);
    }

    #[test]
    fn validate_credentials_accepts_empty_set_for_every_kind() {
        // Ambient mode — always valid, even for kinds with required fields.
        assert!(validate_credentials(BackendKind::S3, &[]).is_ok());
        assert!(validate_credentials(BackendKind::B2, &[]).is_ok());
        assert!(validate_credentials(BackendKind::Local, &[]).is_ok());
        assert!(validate_credentials(BackendKind::Other, &[]).is_ok());
    }

    #[test]
    fn validate_credentials_accepts_complete_s3_set() {
        let creds = kv(&[("AWS_ACCESS_KEY_ID", "id"), ("AWS_SECRET_ACCESS_KEY", "secret")]);
        assert!(validate_credentials(BackendKind::S3, &creds).is_ok());
    }

    #[test]
    fn validate_credentials_accepts_complete_b2_set() {
        let creds = kv(&[("B2_ACCOUNT_ID", "id"), ("B2_ACCOUNT_KEY", "key")]);
        assert!(validate_credentials(BackendKind::B2, &creds).is_ok());
    }

    #[test]
    fn validate_credentials_rejects_missing_required_key() {
        let creds = kv(&[("AWS_ACCESS_KEY_ID", "id")]); // missing secret
        let err = validate_credentials(BackendKind::S3, &creds).unwrap_err();
        assert!(err.contains("AWS_SECRET_ACCESS_KEY"));
    }

    #[test]
    fn validate_credentials_rejects_blank_required_value() {
        let creds = kv(&[("AWS_ACCESS_KEY_ID", "id"), ("AWS_SECRET_ACCESS_KEY", "   ")]);
        let err = validate_credentials(BackendKind::S3, &creds).unwrap_err();
        assert!(err.contains("AWS_SECRET_ACCESS_KEY"));
    }

    #[test]
    fn validate_credentials_rejects_unknown_key_for_s3_and_b2() {
        let creds = kv(&[
            ("AWS_ACCESS_KEY_ID", "id"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("SOME_OTHER_VAR", "x"),
        ]);
        assert!(validate_credentials(BackendKind::S3, &creds).is_err());
    }

    #[test]
    fn validate_credentials_allows_arbitrary_keys_for_other() {
        let creds = kv(&[("MY_CUSTOM_TOKEN", "x")]);
        assert!(validate_credentials(BackendKind::Other, &creds).is_ok());
    }

    #[test]
    fn validate_credentials_rejects_path_for_every_kind() {
        let creds = kv(&[("PATH", "/evil")]);
        for kind in [BackendKind::Local, BackendKind::S3, BackendKind::B2, BackendKind::Rest, BackendKind::Other] {
            assert!(validate_credentials(kind, &creds).is_err(), "{kind:?} should reject PATH");
        }
    }

    #[test]
    fn validate_credentials_rejects_restic_prefixed_keys_for_every_kind() {
        let creds = kv(&[("RESTIC_PASSWORD", "hunter2")]);
        for kind in [BackendKind::Local, BackendKind::S3, BackendKind::B2, BackendKind::Rest, BackendKind::Other] {
            assert!(
                validate_credentials(kind, &creds).is_err(),
                "{kind:?} should reject RESTIC_* keys"
            );
        }
    }

    // ── REST backend (RESTIC_REST_USERNAME / RESTIC_REST_PASSWORD) ─────────

    #[test]
    fn validate_credentials_accepts_rest_auth_pair() {
        let creds = kv(&[("RESTIC_REST_USERNAME", "u"), ("RESTIC_REST_PASSWORD", "pass/word")]);
        assert!(validate_credentials(BackendKind::Rest, &creds).is_ok());
    }

    #[test]
    fn validate_credentials_accepts_rest_username_or_password_alone() {
        // Both specs are optional — a server with only a password, or neither.
        assert!(validate_credentials(BackendKind::Rest, &kv(&[("RESTIC_REST_PASSWORD", "p")])).is_ok());
        assert!(validate_credentials(BackendKind::Rest, &[]).is_ok());
    }

    #[test]
    fn validate_credentials_allows_arbitrary_keys_for_rest() {
        // A rest: repo predating BackendKind::Rest was Other and may already store an
        // arbitrary key. Adding Rest to the S3/B2 unknown-key check would break it on
        // the next edit — this test is what makes that regression loud.
        assert!(validate_credentials(BackendKind::Rest, &kv(&[("HTTPS_PROXY", "x")])).is_ok());
    }

    #[test]
    fn validate_credentials_still_rejects_other_restic_keys_for_rest() {
        for key in ["RESTIC_PASSWORD", "RESTIC_REPOSITORY", "RESTIC_FROM_PASSWORD", "RESTIC_COMPRESSION"] {
            let err = validate_credentials(BackendKind::Rest, &kv(&[(key, "x")])).unwrap_err();
            assert!(err.contains("reserved"), "{key} should still be reserved");
        }
    }

    #[test]
    fn validate_credentials_rejects_rest_keys_for_s3_and_b2() {
        // The allowlist widens is_reserved_key for every kind, but S3/B2's unknown-key
        // check must still refuse a REST credential on a bucket repo.
        let creds = kv(&[
            ("AWS_ACCESS_KEY_ID", "id"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("RESTIC_REST_USERNAME", "u"),
        ]);
        assert!(validate_credentials(BackendKind::S3, &creds).is_err());
    }

    #[test]
    fn validate_credentials_still_rejects_duplicate_rest_keys() {
        let creds = kv(&[("RESTIC_REST_PASSWORD", "a"), ("RESTIC_REST_PASSWORD", "b")]);
        assert!(validate_credentials(BackendKind::Rest, &creds).unwrap_err().contains("more than once"));
    }

    // ── is_reserved_key ─────────────────────────────────────────────────────

    #[test]
    fn is_reserved_key_matches_path_and_restic_prefix() {
        assert!(is_reserved_key("PATH"));
        assert!(is_reserved_key("RESTIC_PASSWORD"));
        assert!(is_reserved_key("RESTIC_REPOSITORY"));
        assert!(!is_reserved_key("AWS_ACCESS_KEY_ID"));
        assert!(!is_reserved_key("B2_ACCOUNT_ID"));
        // Guards against a `starts_with("PATH")` slip — only the exact "PATH" is reserved.
        assert!(!is_reserved_key("PATHOLOGICAL"));
        // Allowlisted — the app never sets these, so a stored value has no collision to win.
        assert!(!is_reserved_key("RESTIC_REST_USERNAME"));
        assert!(!is_reserved_key("RESTIC_REST_PASSWORD"));
        // Neighbours of the allowlist stay reserved — it is an exact-match list, not a prefix.
        assert!(is_reserved_key("RESTIC_REST_FOO"));
        assert!(is_reserved_key("RESTIC_REST_USERNAME_2"));
    }

    #[test]
    fn loader_env_vars_are_reserved() {
        for key in ["LD_PRELOAD", "LD_LIBRARY_PATH", "LD_AUDIT", "DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH"] {
            assert!(is_reserved_key(key), "{key} should be reserved");
        }
        // Prefix, not substring — unrelated names that merely contain "LD_" stay allowed.
        assert!(!is_reserved_key("OLD_TOKEN"));
        assert!(!is_reserved_key("SOLD_OUT"));
    }

    #[test]
    fn reserved_keys_match_case_insensitively() {
        // Windows env names are case-insensitive, so these would collide there.
        assert!(is_reserved_key("Path"));
        assert!(is_reserved_key("restic_repository"));
        assert!(is_reserved_key("Restic_Password"));
        assert!(is_reserved_key("ld_preload"));
        // The allowlist is case-insensitive too, and lowercase proxy vars remain fine.
        assert!(!is_reserved_key("restic_rest_password"));
        assert!(!is_reserved_key("https_proxy"));
    }

    #[test]
    fn validate_credentials_rejects_ld_preload() {
        let err = validate_credentials(BackendKind::Rest, &kv(&[("LD_PRELOAD", "/tmp/evil.so")])).unwrap_err();
        assert!(err.contains("reserved"));
        assert!(validate_credentials(BackendKind::Other, &kv(&[("DYLD_INSERT_LIBRARIES", "x")])).is_err());
    }

    #[test]
    fn https_proxy_still_allowed_for_rest_and_other() {
        // Pins the settled arbitrary-keys allowance — the loader denylist must not widen
        // into a general allowlist.
        assert!(validate_credentials(BackendKind::Rest, &kv(&[("HTTPS_PROXY", "http://p:3128")])).is_ok());
        assert!(validate_credentials(BackendKind::Other, &kv(&[("HTTPS_PROXY", "http://p:3128")])).is_ok());
    }

    // ── duplicate keys ──────────────────────────────────────────────────────

    #[test]
    fn validate_credentials_rejects_duplicate_keys() {
        let creds = kv(&[("B2_ACCOUNT_ID", "one"), ("B2_ACCOUNT_ID", "two")]);
        let err = validate_credentials(BackendKind::B2, &creds).unwrap_err();
        assert!(err.contains("B2_ACCOUNT_ID"));
    }

    #[test]
    fn validate_credentials_reports_duplicate_before_missing_required() {
        // Two AWS_ACCESS_KEY_ID rows, no AWS_SECRET_ACCESS_KEY at all — the duplicate
        // should be caught first rather than surfacing as a confusing "missing
        // required credential" error.
        let creds = kv(&[("AWS_ACCESS_KEY_ID", "one"), ("AWS_ACCESS_KEY_ID", "two")]);
        let err = validate_credentials(BackendKind::S3, &creds).unwrap_err();
        assert!(err.contains("more than once"));
    }

    // ── rest_error_hint / append_rest_hint ──────────────────────────────────

    const REST: &str = "rest:https://nas:8000/alice/";

    #[test]
    fn hint_for_unknown_certificate_authority() {
        let err = "Fatal: create repository at rest:https://127.0.0.1:8443/repo/ failed: Post \
                   \"https://:***@127.0.0.1:8443/repo/?create=true\": tls: failed to verify \
                   certificate: x509: certificate signed by unknown authority";
        let hint = rest_error_hint(REST, false, err).unwrap();
        assert!(hint.contains("certificate store"));
        assert!(hint.contains("--insecure-tls"));
        // With verification already off, the checkbox can't help — no hint.
        assert_eq!(rest_error_hint(REST, true, err), None);
    }

    #[test]
    fn hint_for_401_mentions_private_repos() {
        let err = "Fatal: unable to open config file: unexpected HTTP response (401): 401 Unauthorized";
        let hint = rest_error_hint(REST, false, err).unwrap();
        assert!(hint.contains("401"));
        assert!(hint.contains("--private-repos"));
    }

    #[test]
    fn hint_for_missing_repository() {
        let err = "Fatal: repository does not exist: unable to open config file: <config/> does not exist";
        assert!(rest_error_hint(REST, false, err).unwrap().contains("404"));
    }

    #[test]
    fn hint_for_forbidden_names_append_only() {
        let err = "Remove(<snapshot/45cea74811>) failed: unexpected HTTP response (403): 403 Forbidden";
        let hint = rest_error_hint(REST, false, err).unwrap();
        assert!(hint.contains("--append-only"));
        // The hint is appended to any failed `rest:` call, so it also names the other usual cause.
        assert!(hint.contains("access rule or proxy"), "{hint}");
    }

    #[test]
    fn http_status_hints_ignore_status_numbers_in_file_names_and_os_messages() {
        // A failed backup's stderr carries paths and OS errors. A name like `report (404).pdf`
        // is not the server's answer and must not produce a "Repository not found" (etc.) hint.
        for err in [
            "error: open /home/u/report (404).pdf: permission denied",
            "error: open /srv/invoice (401).txt: permission denied",
            "error: open /srv/photo (403).jpg: no such file or directory",
            "error: open /srv/dump (507).bin: input/output error",
            "error: open /srv/log 400 Bad Request.txt: permission denied",
            "error: open /srv/notes 403 Forbidden.md: permission denied",
        ] {
            assert_eq!(rest_error_hint(REST, false, err), None, "{err}");
            assert_eq!(rest_error_hint("rest:http://nas:8000/r", false, err), None, "{err}");
        }
        // …while the same statuses in restic's own shape still explain themselves.
        for (code, needle) in [(401, "Authentication failed"), (403, "refused the request"), (404, "Repository not found"), (507, "out of storage")] {
            let err = format!("error: open /srv/report ({code}).pdf: denied\nFatal: unexpected HTTP response ({code}): {code} Reason");
            assert!(rest_error_hint(REST, false, &err).unwrap().contains(needle), "{code}");
        }
    }

    #[test]
    fn hint_for_storage_full() {
        let err = "Fatal: unable to save snapshot: error flushing repository: unexpected HTTP response (507): 507 Insufficient Storage";
        assert!(rest_error_hint(REST, false, err).unwrap().contains("--max-size"));
    }

    #[test]
    fn hint_for_scheme_mismatches() {
        let https_to_http = "Head \"https://:***@127.0.0.1:8503/repo/config\": http: server gave HTTP response to HTTPS client";
        assert!(rest_error_hint(REST, false, https_to_http).unwrap().contains("rest:http://"));

        let http_to_https = "Stat(<config/>) returned error, retrying after 1.7s: unexpected HTTP response (400): 400 Bad Request";
        // 400 is only explained for a plain-HTTP URL.
        assert!(rest_error_hint("rest:http://nas:8000/r", false, http_to_https).unwrap().contains("rest:https://"));
        assert!(rest_error_hint(REST, false, "unexpected HTTP response (400): 400 Bad Request").is_none());
    }

    #[test]
    fn hint_for_unreachable_server() {
        for err in [
            "Head \"http://:***@127.0.0.1:9/x/config\": dial tcp 127.0.0.1:9: connect: connection refused",
            "dial tcp: lookup nonexistent.invalid: no such host",
            "Timed out waiting for the server after 30 seconds",
            "Stat(<config/>) returned error, retrying after 2.8s: something",
        ] {
            assert!(rest_error_hint(REST, false, err).unwrap().contains("reach"), "{err}");
        }
    }

    #[test]
    fn no_hint_for_non_rest_repos_or_unrecognized_errors() {
        let err = "unexpected HTTP response (401): 401 Unauthorized";
        assert_eq!(rest_error_hint("s3:s3.amazonaws.com/b", false, err), None);
        assert_eq!(rest_error_hint("/local/repo", false, err), None);
        assert_eq!(rest_error_hint(REST, false, "Fatal: wrong password or no key found"), None);
    }

    #[test]
    fn append_rest_hint_appends_and_preserves_the_original_text() {
        let err = "Fatal: unable to open config file: unexpected HTTP response (401): 401 Unauthorized\n".to_string();
        let out = append_rest_hint(REST, false, err.clone());
        assert!(out.starts_with("Fatal: unable to open config file"));
        assert!(out.contains("\n\nHint: Authentication failed"));
        // Unrecognized: byte-for-byte unchanged.
        let other = "Fatal: wrong password or no key found".to_string();
        assert_eq!(append_rest_hint(REST, false, other.clone()), other);
    }
}
