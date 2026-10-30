//! The network checks of `ferrite doctor`: `smtp.connect`, `tls.acme_directory`,
//! `net.listen` and `release.latest` (P50, P51). SMTP, the ACME directory and the release
//! lookup reach the outside, each within 5 s, and are skipped with `--offline`; `net.listen`
//! only talks to this machine's running node. The SMTP URL and its user never appear.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use ferrite_config::{AcmeDirectory, Config, TlsConfig};

use super::Finding;
use super::fs::NO_CONFIG;
use crate::update::Latest;

/// The bound on each network check (P51).
const NET_TIMEOUT: Duration = Duration::from_secs(5);

/// `smtp.connect`: with `[mail]`, a connection as a send makes it (TLS, EHLO, AUTH) and a
/// `NOOP`, no mail sent; a failure is a warning (mail failing does not stop the site).
pub(super) async fn smtp(config: Option<&Config>, offline: bool) -> Finding {
    const ID: &str = "smtp.connect";
    let Some(config) = config else {
        return Finding::skip(ID, NO_CONFIG);
    };
    let Some(mail) = &config.mail else {
        return Finding::info(ID, "mail is off ([mail] smtp_url is not set)");
    };
    let target = smtp_target(mail.smtp_url.expose());
    if offline {
        return Finding::skip(ID, format!("{target}: not tried (--offline)"));
    }
    let mailer = match ferrite_http::mail::SmtpMailer::new(mail) {
        Ok(mailer) => mailer,
        Err(e) => return Finding::warn(ID, format!("{target}: {e}")),
    };
    match tokio::time::timeout(NET_TIMEOUT, mailer.test_connection()).await {
        Ok(Ok(())) => Finding::ok(ID, format!("{target}: the server accepted a session")),
        Ok(Err(e)) => Finding::warn(ID, format!("{target}: {e}")),
        Err(_) => Finding::warn(
            ID,
            format!("{target}: no answer within {} s", NET_TIMEOUT.as_secs()),
        ),
    }
}

/// `host:port (TLS mode)` of an SMTP URL, with the port lettre uses when none is given;
/// never the user or the password.
fn smtp_target(url: &str) -> String {
    let Ok(u) = url::Url::parse(url) else {
        return "mail.smtp_url".to_owned();
    };
    let required = u.query_pairs().any(|(k, v)| k == "tls" && v == "required");
    let (mode, port) = match u.scheme() {
        "smtps" => ("implicit TLS", 465),
        _ if required => ("STARTTLS required", 587),
        _ => ("plain, no TLS", 25),
    };
    let host = u.host_str().unwrap_or("?");
    format!("{host}:{} ({mode})", u.port().unwrap_or(port))
}

/// `tls.acme_directory`: in ACME mode the directory URL answers with its JSON (a `GET`, as
/// the ACME client's first request); a failure is a warning (the current certificate keeps
/// serving until it expires).
pub(super) async fn acme_directory(config: Option<&Config>, offline: bool) -> Finding {
    const ID: &str = "tls.acme_directory";
    let Some(config) = config else {
        return Finding::skip(ID, NO_CONFIG);
    };
    let TlsConfig::Acme(acme) = &config.server.tls else {
        return Finding::info(ID, "TLS is off (server.tls.mode)");
    };
    let url = acme.directory.url();
    let which = match acme.directory {
        AcmeDirectory::Production => "Let's Encrypt",
        AcmeDirectory::Staging => "Let's Encrypt staging: untrusted certificates",
        AcmeDirectory::Url(_) => "a custom CA",
    };
    if offline {
        return Finding::skip(ID, format!("{url} ({which}): not fetched (--offline)"));
    }
    match tokio::time::timeout(NET_TIMEOUT, fetch_directory(url)).await {
        Ok(Ok(())) => Finding::ok(ID, format!("{url} ({which}) answered")),
        Ok(Err(e)) => Finding::warn(
            ID,
            format!("{url} ({which}): {e}; certificates cannot be issued or renewed"),
        ),
        Err(_) => Finding::warn(
            ID,
            format!(
                "{url} ({which}): no answer within {} s; certificates cannot be issued or \
                 renewed",
                NET_TIMEOUT.as_secs()
            ),
        ),
    }
}

/// `GET url`: a 2xx JSON object naming `newNonce` (RFC 8555 §7.1.1).
async fn fetch_directory(url: &str) -> Result<(), String> {
    // reqwest has no TLS provider of its own (M4-2 P28); an installed one is kept.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .user_agent(concat!("ferrite/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .build()
        .map_err(|e| format!("cannot build the HTTP client: {e}"))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("cannot reach it: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("it answered {status}"));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| format!("reading the answer failed: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| "the answer is not JSON".to_owned())?;
    if json.get("newNonce").is_none() {
        return Err("the answer is not an ACME directory (no newNonce)".to_owned());
    }
    Ok(())
}

/// Whether a node is running for this data folder, as far as doctor can tell (P50).
pub(super) enum Node {
    /// SQLite: the data-folder lock is held. Postgres: a live node row has this
    /// configuration's `cluster.node_name`.
    Running,
    /// Why doctor cannot tell, or that none runs.
    NotRunning(&'static str),
}

/// `net.listen` (P50): with a node running, `/healthz` and `/readyz` on `server.listen`
/// (and `/metrics` on `server.metrics_listen`) must answer 200. Without one, `skip`.
pub(super) async fn listen(config: Option<&Config>, node: Node) -> Finding {
    const ID: &str = "net.listen";
    let Some(config) = config else {
        return Finding::skip(ID, NO_CONFIG);
    };
    if let Node::NotRunning(why) = node {
        return Finding::skip(ID, why);
    }
    let addr = reachable(config.server.listen);
    if addr.port() == 0 {
        return Finding::skip(
            ID,
            "server.listen has port 0 (a random port): its address is not known",
        );
    }
    let base = format!("http://{addr}");
    let mut seen = Vec::new();
    for path in ["/healthz", "/readyz"] {
        let url = format!("{base}{path}");
        match tokio::time::timeout(NET_TIMEOUT, crate::healthcheck::probe(&url)).await {
            Ok(Ok(200)) => seen.push(format!("{path} 200")),
            Ok(Ok(status)) if path == "/readyz" => {
                return Finding::error(
                    ID,
                    format!(
                        "{url} answered {status}: the node is up but not ready (it cannot \
                         reach its database)"
                    ),
                );
            }
            Ok(Ok(status)) => {
                return Finding::error(ID, format!("{url} answered {status}"));
            }
            Ok(Err(e)) => {
                return Finding::warn(
                    ID,
                    format!(
                        "{url}: {e:#} (the process running here may not be `serve`, or it \
                         listens on another address)"
                    ),
                );
            }
            Err(_) => {
                return Finding::warn(
                    ID,
                    format!("{url}: no answer within {} s", NET_TIMEOUT.as_secs()),
                );
            }
        }
    }
    let mut detail = format!("{base}: {}", seen.join(", "));
    if let Some(metrics) = config.server.metrics_listen {
        let url = format!("http://{}/metrics", reachable(metrics));
        match tokio::time::timeout(NET_TIMEOUT, crate::healthcheck::probe(&url)).await {
            Ok(Ok(200)) => detail.push_str(&format!("; {url} 200")),
            Ok(Ok(status)) => {
                return Finding::warn(ID, format!("{detail}; {url} answered {status}"));
            }
            Ok(Err(e)) => return Finding::warn(ID, format!("{detail}; {url}: {e:#}")),
            Err(_) => {
                return Finding::warn(
                    ID,
                    format!(
                        "{detail}; {url}: no answer within {} s",
                        NET_TIMEOUT.as_secs()
                    ),
                );
            }
        }
    }
    Finding::ok(ID, detail)
}

/// A listen address to connect to: the loopback address of its family for a wildcard.
fn reachable(addr: SocketAddr) -> SocketAddr {
    match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), addr.port())
        }
        _ => addr,
    }
}

/// `release.latest` (P51): the newest stable release against this binary, looked up as
/// `self-update --check` does. Newer is a warning; a failed lookup only `info`, never an
/// error. `test_base` is the hidden `--base-url` seam.
pub(super) async fn release(offline: bool, test_base: Option<&str>) -> Finding {
    const ID: &str = "release.latest";
    if offline {
        return Finding::skip(ID, "the release lookup is not made (--offline)");
    }
    let current = env!("CARGO_PKG_VERSION");
    match tokio::time::timeout(NET_TIMEOUT, crate::update::latest_release(test_base)).await {
        Ok(Ok(Latest::Newer(v))) => Finding::warn(
            ID,
            format!(
                "a newer release is available: {v} (this binary: {current}); `ferrite \
                 self-update` installs it"
            ),
        ),
        Ok(Ok(Latest::UpToDate(v))) => Finding::ok(
            ID,
            format!("ferrite {current} is up to date (newest release: {v})"),
        ),
        Ok(Ok(Latest::Prerelease(tag))) => Finding::info(
            ID,
            format!("the newest release {tag} is a prerelease; the stable channel skips it"),
        ),
        Ok(Err(e)) => Finding::info(ID, format!("could not look up the newest release: {e:#}")),
        Err(_) => Finding::info(
            ID,
            format!(
                "the release lookup did not answer within {} s",
                NET_TIMEOUT.as_secs()
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smtp_target_names_host_port_and_mode_only() {
        assert_eq!(
            smtp_target("smtps://u:p@mail.example.com"),
            "mail.example.com:465 (implicit TLS)"
        );
        assert_eq!(
            smtp_target("smtp://u:p@mail.example.com:2525?tls=required"),
            "mail.example.com:2525 (STARTTLS required)"
        );
        assert_eq!(
            smtp_target("smtp://127.0.0.1"),
            "127.0.0.1:25 (plain, no TLS)"
        );
        assert!(!smtp_target("smtps://user:secret@h").contains("secret"));
    }

    #[test]
    fn wildcard_listen_addresses_are_probed_on_loopback() {
        let any: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        assert_eq!(reachable(any), "127.0.0.1:8080".parse().unwrap());
        let any6: SocketAddr = "[::]:8080".parse().unwrap();
        assert_eq!(reachable(any6), "[::1]:8080".parse().unwrap());
        let one: SocketAddr = "10.0.0.5:80".parse().unwrap();
        assert_eq!(reachable(one), one);
    }
}
