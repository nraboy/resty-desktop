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

/// The only `RESTIC_*` vars a stored credential may set. Unlike RESTIC_REPOSITORY /
/// RESTIC_PASSWORD / RESTIC_FROM_* / RESTIC_COMPRESSION, the app never sets these
/// itself, so there is no collision for a stored value to win and nothing to
/// redirect — they only select HTTP basic auth for the REST backend. Kept as an
/// explicit allowlist rather than a `RESTIC_REST_` prefix match so a future
/// `RESTIC_REST_*` var restic may add doesn't become settable by accident.
const ALLOWED_RESTIC_KEYS: &[&str] = &["RESTIC_REST_USERNAME", "RESTIC_REST_PASSWORD"];

/// Env var names the app controls itself and a stored credential must never set:
/// `PATH` (see `NoConsole::augment_path`) and every `RESTIC_*` var except the two
/// REST auth vars in `ALLOWED_RESTIC_KEYS` (repository, password, compression, …
/// all stay reserved). Shared by `validate_credentials` — which rejects such a
/// key at entry, the earlier and louder guard — and `repo::apply_backend_env`, which
/// skips it at apply time so the guarantee holds even for a credential that reached
/// the DB some other way (e.g. a hand-edited import bundle).
pub fn is_reserved_key(key: &str) -> bool {
    key == "PATH" || (key.starts_with("RESTIC_") && !ALLOWED_RESTIC_KEYS.contains(&key))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
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
}
