//! `ferrite healthcheck`: a dependency-free HTTP probe for container health checks
//! (the distroless image has no shell, curl or wget).

use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use clap::Args;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Whole probe: connect, send, read the status line.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Args)]
pub struct HealthcheckArgs {
    /// Plain http:// URL to GET; exit 0 only on a 200 response.
    #[arg(long, default_value = "http://127.0.0.1:8080/readyz")]
    pub url: String,
}

pub async fn run(args: HealthcheckArgs) -> anyhow::Result<ExitCode> {
    match tokio::time::timeout(PROBE_TIMEOUT, probe(&args.url)).await {
        Ok(Ok(200)) => Ok(ExitCode::SUCCESS),
        Ok(Ok(status)) => {
            eprintln!("unhealthy: HTTP {status}");
            Ok(ExitCode::FAILURE)
        }
        Ok(Err(e)) => {
            eprintln!("unhealthy: {e:#}");
            Ok(ExitCode::FAILURE)
        }
        Err(_) => {
            eprintln!("unhealthy: no response within {PROBE_TIMEOUT:?}");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// GETs `url` over HTTP/1.0 and returns the response status code.
async fn probe(url: &str) -> anyhow::Result<u16> {
    let url = url::Url::parse(url).context("invalid --url")?;
    anyhow::ensure!(url.scheme() == "http", "--url must be an http:// URL");
    let host = url.host_str().context("--url has no host")?;
    let port = url.port_or_known_default().unwrap_or(80);
    let mut target = url.path().to_owned();
    if let Some(q) = url.query() {
        target.push('?');
        target.push_str(q);
    }
    let mut stream = TcpStream::connect((host, port))
        .await
        .with_context(|| format!("cannot connect to {host}:{port}"))?;
    let authority = match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    };
    let request =
        format!("GET {target} HTTP/1.0\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut head = Vec::new();
    let mut buf = [0u8; 256];
    while !head.contains(&b'\n') && head.len() < 1024 {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        head.extend_from_slice(&buf[..n]);
    }
    let line = String::from_utf8_lossy(&head);
    let status = line
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .context("malformed HTTP response")?;
    Ok(status)
}
