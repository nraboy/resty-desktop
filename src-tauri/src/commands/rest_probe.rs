//! Detects whether a `rest:` repository's server runs `rest-server --append-only`.
//!
//! restic can't tell us: it never issues a `DELETE` for a blob it doesn't own, and every
//! command that would (forget, prune) is destructive. rest-server, however, checks
//! `AppendOnly` in `deleteBlob` **before** it looks at the file (`repo/repo.go`), so
//! `DELETE <repo>/data/<64 hex>` answers:
//!
//! - **403** under `--append-only`, even for an id that doesn't exist;
//! - **200** on a normal server, where deleting a missing file is an idempotent no-op.
//!
//! The probe id is 64 zeros. restic names objects by the SHA-256 of their content, so no real
//! blob can carry that name, and the request can never delete anything.
//!
//! Auth, a `--private-repos` path mismatch and a missing repo answer 401/404 instead, so the
//! probe is only meaningful **after** a successful connection test (the UI runs it in that
//! order); any other answer is reported as undetermined rather than guessed at.
//!
//! The request goes out over `ureq` + rustls with the platform's root certificates
//! (`native-certs`), matching restic, which also trusts the OS store. A repo flagged
//! `insecure_tls` is probed with certificate verification off, matching its restic calls.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

/// 64 hex zeros — a syntactically valid blob id that no real object can have.
pub(crate) const PROBE_BLOB_ID: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProbeStatus {
    AppendOnly,
    NotAppendOnly,
    Undetermined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppendOnlyProbe {
    pub status: ProbeStatus,
    /// One line the UI can show as-is.
    pub detail: String,
}

impl AppendOnlyProbe {
    fn new(status: ProbeStatus, detail: impl Into<String>) -> Self {
        Self { status, detail: detail.into() }
    }
    fn undetermined(detail: impl Into<String>) -> Self {
        Self::new(ProbeStatus::Undetermined, detail)
    }
}

/// Maps the HTTP status of the probe `DELETE` to a verdict. Pure.
pub(crate) fn interpret_status(code: u16) -> AppendOnlyProbe {
    match code {
        403 => AppendOnlyProbe::new(
            ProbeStatus::AppendOnly,
            "The server refuses deletes: it is running in append-only mode.",
        ),
        200..=299 => AppendOnlyProbe::new(
            ProbeStatus::NotAppendOnly,
            "The server allows deletes: it is not running in append-only mode.",
        ),
        401 => AppendOnlyProbe::undetermined(
            "The server rejected the credentials (401), so append-only mode couldn't be checked.",
        ),
        404 => AppendOnlyProbe::undetermined(
            "The repository path wasn't found (404), so append-only mode couldn't be checked.",
        ),
        other => AppendOnlyProbe::undetermined(format!(
            "The server answered HTTP {other}, so append-only mode couldn't be checked."
        )),
    }
}

/// Where to send the probe and how to authenticate it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ProbeTarget {
    pub url: String,
    /// Full `Authorization` header value, when credentials are known.
    pub authorization: Option<String>,
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Builds the probe request for a `rest:http(s)://…` repository path. Credentials follow
/// restic's own precedence: inline `user:pass@` userinfo wins; otherwise the stored
/// `RESTIC_REST_USERNAME`/`RESTIC_REST_PASSWORD` pair (either may be absent). Pure.
pub(crate) fn build_target(path: &str, credentials: &[(String, String)]) -> Result<ProbeTarget, String> {
    use base64::Engine as _;

    let raw = path.trim().strip_prefix("rest:").ok_or("Not a rest: repository.")?;
    let mut url = url::Url::parse(raw).map_err(|e| format!("Couldn't parse the repository URL: {e}"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(format!("Append-only detection doesn't support {}:// URLs.", url.scheme()));
    }

    let stored = |key: &str| {
        credentials
            .iter()
            .find(|(k, v)| k == key && !v.is_empty())
            .map(|(_, v)| v.clone())
    };
    let (user, pass) = if !url.username().is_empty() || url.password().is_some() {
        (
            Some(percent_decode(url.username())),
            url.password().map(percent_decode),
        )
    } else {
        (stored("RESTIC_REST_USERNAME"), stored("RESTIC_REST_PASSWORD"))
    };
    let authorization = if user.is_some() || pass.is_some() {
        let token = format!("{}:{}", user.unwrap_or_default(), pass.unwrap_or_default());
        Some(format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(token)))
    } else {
        None
    };

    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    let base = url.path().trim_end_matches('/').to_string();
    url.set_path(&format!("{base}/data/{PROBE_BLOB_ID}"));

    Ok(ProbeTarget { url: url.to_string(), authorization })
}

/// rustls verifier that accepts any server certificate — used only for a repository the user
/// explicitly flagged `insecure_tls`, mirroring the `--insecure-tls` its restic calls carry.
#[derive(Debug)]
struct AcceptAnyCert(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn insecure_tls_config() -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCert(provider)))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Sends the probe and interprets the answer. Blocking — callers run it under
/// `spawn_blocking`.
pub(crate) fn probe(
    path: &str,
    insecure_tls: bool,
    credentials: &[(String, String)],
) -> AppendOnlyProbe {
    let target = match build_target(path, credentials) {
        Ok(t) => t,
        Err(e) => return AppendOnlyProbe::undetermined(e),
    };
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        // A redirect would turn the DELETE into something else entirely.
        .redirects(0);
    if insecure_tls {
        match insecure_tls_config() {
            Ok(cfg) => builder = builder.tls_config(cfg),
            Err(e) => return AppendOnlyProbe::undetermined(format!("Couldn't set up TLS: {e}")),
        }
    }
    let mut req = builder.build().delete(&target.url);
    if let Some(auth) = &target.authorization {
        req = req.set("Authorization", auth);
    }
    match req.call() {
        Ok(resp) => interpret_status(resp.status()),
        Err(ureq::Error::Status(code, _)) => interpret_status(code),
        Err(ureq::Error::Transport(t)) => {
            AppendOnlyProbe::undetermined(format!(
                "Couldn't check append-only mode ({t}). You can set it manually."
            ))
        }
    }
}

/// Checks a `rest:` repository for `--append-only`. Call it after a successful connection
/// test (see the module docs); it never changes any state itself.
#[tauri::command]
pub async fn detect_append_only(
    path: String,
    insecure_tls: bool,
    credentials: Vec<(String, String)>,
) -> Result<AppendOnlyProbe, String> {
    tauri::async_runtime::spawn_blocking(move || probe(&path, insecure_tls, &credentials))
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn creds(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn basic(user_pass: &str) -> Option<String> {
        use base64::Engine as _;
        Some(format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(user_pass)))
    }

    #[test]
    fn probe_id_is_a_valid_blob_id_no_real_object_has() {
        assert_eq!(PROBE_BLOB_ID.len(), 64);
        assert!(PROBE_BLOB_ID.chars().all(|c| c == '0'));
    }

    #[test]
    fn interpret_status_maps_the_answers_rest_server_gives() {
        assert_eq!(interpret_status(403).status, ProbeStatus::AppendOnly);
        assert_eq!(interpret_status(200).status, ProbeStatus::NotAppendOnly);
        assert_eq!(interpret_status(204).status, ProbeStatus::NotAppendOnly);
        for code in [401, 404, 400, 500, 507] {
            assert_eq!(interpret_status(code).status, ProbeStatus::Undetermined, "{code}");
        }
    }

    #[test]
    fn build_target_targets_the_zero_blob_under_the_repo_path() {
        let t = build_target("rest:https://nas:8000/alice/", &[]).unwrap();
        assert_eq!(t.url, format!("https://nas:8000/alice/data/{PROBE_BLOB_ID}"));
        assert_eq!(t.authorization, None);
        // No trailing slash, no path, and a query/fragment are all normalized.
        assert_eq!(
            build_target("rest:http://h:1/repo?x=1#f", &[]).unwrap().url,
            format!("http://h:1/repo/data/{PROBE_BLOB_ID}")
        );
        assert_eq!(
            build_target("rest:http://h:1", &[]).unwrap().url,
            format!("http://h:1/data/{PROBE_BLOB_ID}")
        );
    }

    #[test]
    fn build_target_uses_inline_userinfo_percent_decoded_and_strips_it_from_the_url() {
        let t = build_target("rest:https://al%40ice:p%2Fw%25d@nas/r", &creds(&[("RESTIC_REST_USERNAME", "other")])).unwrap();
        assert_eq!(t.authorization, basic("al@ice:p/w%d"));
        assert!(!t.url.contains('@'));
        assert!(!t.url.contains("p%2Fw"));
    }

    #[test]
    fn build_target_falls_back_to_stored_rest_credentials() {
        let t = build_target(
            "rest:https://nas/r",
            &creds(&[("RESTIC_REST_USERNAME", "bob"), ("RESTIC_REST_PASSWORD", "secret2")]),
        )
        .unwrap();
        assert_eq!(t.authorization, basic("bob:secret2"));
        // Either half alone is enough, like restic.
        let only_user = build_target("rest:https://nas/r", &creds(&[("RESTIC_REST_USERNAME", "bob")])).unwrap();
        assert_eq!(only_user.authorization, basic("bob:"));
        // Empty stored values are ignored (apply_backend_env skips them too).
        let empty = build_target("rest:https://nas/r", &creds(&[("RESTIC_REST_USERNAME", "")])).unwrap();
        assert_eq!(empty.authorization, None);
    }

    #[test]
    fn build_target_rejects_what_it_cannot_probe() {
        assert!(build_target("/local/repo", &[]).is_err());
        assert!(build_target("s3:s3.amazonaws.com/b", &[]).is_err());
        assert!(build_target("rest:http+unix:///tmp/rest.sock:/repo", &[]).is_err());
        assert!(build_target("rest:not a url", &[]).is_err());
    }

    /// One-shot HTTP server: answers the first request with `status_line`, then hands the
    /// raw request text back for assertions.
    fn serve_once(status_line: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let body = "";
            write!(sock, "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            String::from_utf8_lossy(&buf).into_owned()
        });
        (format!("127.0.0.1:{}", addr.port()), handle)
    }

    #[test]
    fn probe_detects_append_only_on_403_and_sends_a_delete_with_basic_auth() {
        let (addr, server) = serve_once("403 Forbidden");
        let result = probe(&format!("rest:http://alice:secret1@{addr}/alice"), false, &[]);
        assert_eq!(result.status, ProbeStatus::AppendOnly, "{}", result.detail);
        let request = server.join().unwrap();
        assert!(request.starts_with(&format!("DELETE /alice/data/{PROBE_BLOB_ID} HTTP/1.1")), "{request}");
        use base64::Engine as _;
        let expected = base64::engine::general_purpose::STANDARD.encode("alice:secret1");
        assert!(request.to_ascii_lowercase().contains(&format!("authorization: basic {}", expected.to_ascii_lowercase())), "{request}");
    }

    #[test]
    fn probe_reports_not_append_only_on_200_and_undetermined_otherwise() {
        let (addr, server) = serve_once("200 OK");
        assert_eq!(probe(&format!("rest:http://{addr}/r"), false, &[]).status, ProbeStatus::NotAppendOnly);
        server.join().unwrap();

        let (addr, server) = serve_once("401 Unauthorized");
        assert_eq!(probe(&format!("rest:http://{addr}/r"), false, &[]).status, ProbeStatus::Undetermined);
        server.join().unwrap();
    }

    #[test]
    fn probe_does_not_follow_redirects() {
        let (addr, server) = serve_once("302 Found");
        assert_eq!(probe(&format!("rest:http://{addr}/r"), false, &[]).status, ProbeStatus::Undetermined);
        server.join().unwrap();
    }

    #[test]
    fn probe_is_undetermined_when_nothing_is_listening() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let r = probe(&format!("rest:http://127.0.0.1:{port}/r"), false, &[]);
        assert_eq!(r.status, ProbeStatus::Undetermined);
        assert!(r.detail.contains("Couldn't check"), "{}", r.detail);
    }

    /// Manual check: prints the verdict for `RESTY_PROBE_TEST_URL` (and `RESTY_PROBE_TEST_INSECURE=1`
    /// to skip certificate verification). Run with `-- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn probe_prints_verdict_for_live_server() {
        let url = std::env::var("RESTY_PROBE_TEST_URL").expect("set RESTY_PROBE_TEST_URL");
        let insecure = std::env::var("RESTY_PROBE_TEST_INSECURE").is_ok();
        println!("VERDICT {url} insecure={insecure} -> {:?}", probe(&url, insecure, &[]));
    }

    /// Manual check against a real TLS rest-server with a self-signed certificate (see
    /// docs/restic.md): `RESTY_PROBE_TEST_URL=rest:https://127.0.0.1:8443/repo cargo test
    /// probe_against_live_tls_server -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn probe_against_live_tls_server() {
        let url = std::env::var("RESTY_PROBE_TEST_URL").expect("set RESTY_PROBE_TEST_URL");
        let strict = probe(&url, false, &[]);
        println!("verify on  -> {strict:?}");
        assert_eq!(strict.status, ProbeStatus::Undetermined, "a self-signed cert must fail verification");
        let lax = probe(&url, true, &[]);
        println!("verify off -> {lax:?}");
        assert_ne!(lax.status, ProbeStatus::Undetermined);
    }
}
