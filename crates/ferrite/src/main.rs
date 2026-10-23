use std::process::ExitCode;

use clap::Parser;
use ferrite::Cli;
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<ExitCode> {
    // Default provider for other rustls consumers (sqlx brings its own). An already
    // installed provider is fine.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let filter = match EnvFilter::try_from_env("FERRITE_LOG") {
        Ok(f) => f,
        Err(e) => {
            if std::env::var_os("FERRITE_LOG").is_some() {
                eprintln!("warning: invalid FERRITE_LOG ({e}); using \"info\"");
            }
            EnvFilter::new("info")
        }
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(ferrite::run(cli));
    // Do not wait for stray blocking work (a render still running after the drain).
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}
