use clap::Parser;
use ferrite::Cli;
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    // sqlx uses rustls with ring; install it as the process default. An already installed
    // provider is fine.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let filter = EnvFilter::try_from_env("FERRITE_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(ferrite::run(cli))
}
