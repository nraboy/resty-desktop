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
    /// Any other remote prefix (sftp:, rest:, azure:, gs:, rclone:, …) or an
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

const EMPTY_SPECS: &[CredentialSpec] = &[];

/// The credential fields restic recognizes for `kind`. `Local` and `Other` have no
/// fixed set — `Other` accepts an arbitrary key/value list (see `validate_credentials`).
pub fn credential_specs(kind: BackendKind) -> &'static [CredentialSpec] {
    match kind {
        BackendKind::S3 => S3_SPECS,
        BackendKind::B2 => B2_SPECS,
        BackendKind::Local | BackendKind::Other => EMPTY_SPECS,
    }
}

/// Same prefix list `isRemoteRepo` uses on the frontend (`src/lib/types.ts`) — kept
/// in sync manually since no test spans the two languages.
const REMOTE_PREFIXES: &[&str] = &["s3:", "sftp:", "rest:", "azure:", "gs:", "b2:", "rclone:"];

/// Derives a repository's backend kind from its path. Total — every path resolves to
/// some kind, so this never fails and there is nothing to migrate for existing repos.
pub fn detect_kind(path: &str) -> BackendKind {
    if path.starts_with("s3:") {
        BackendKind::S3
    } else if path.starts_with("b2:") {
        BackendKind::B2
    } else if REMOTE_PREFIXES.iter().any(|p| path.starts_with(p)) {
        BackendKind::Other
    } else {
        BackendKind::Local
    }
}

/// Validates a proposed credential set for `kind`. An empty set is always valid —
/// that's the "use restic's own credential chain" ambient mode (see CLAUDE.md) — so
/// this only enforces shape once the user has actually entered something.
///
/// Every kind rejects a `PATH` or `RESTIC_*` key: `apply_backend_env` runs before
/// `apply_repo_password`/`augment_path()` specifically so those calls win on a
/// conflict, but rejecting the key outright here means a hostile or accidental entry
/// (e.g. from a free-form "Other" list, or a Backrest import) can never even reach
/// that fallback.
pub fn validate_credentials(kind: BackendKind, creds: &[(String, String)]) -> Result<(), String> {
    if creds.is_empty() {
        return Ok(());
    }

    for (key, _) in creds {
        if key == "PATH" || key.starts_with("RESTIC_") {
            return Err(format!(
                "'{key}' is a reserved variable and cannot be set as a backend credential"
            ));
        }
    }

    let specs = credential_specs(kind);
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
        assert_eq!(detect_kind("rest:http://localhost:8000/"), BackendKind::Other);
        assert_eq!(detect_kind("azure:container:/"), BackendKind::Other);
        assert_eq!(detect_kind("gs:bucket:/"), BackendKind::Other);
        assert_eq!(detect_kind("rclone:remote:path"), BackendKind::Other);
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
        for kind in [BackendKind::Local, BackendKind::S3, BackendKind::B2, BackendKind::Other] {
            assert!(validate_credentials(kind, &creds).is_err(), "{kind:?} should reject PATH");
        }
    }

    #[test]
    fn validate_credentials_rejects_restic_prefixed_keys_for_every_kind() {
        let creds = kv(&[("RESTIC_PASSWORD", "hunter2")]);
        for kind in [BackendKind::Local, BackendKind::S3, BackendKind::B2, BackendKind::Other] {
            assert!(
                validate_credentials(kind, &creds).is_err(),
                "{kind:?} should reject RESTIC_* keys"
            );
        }
    }
}
