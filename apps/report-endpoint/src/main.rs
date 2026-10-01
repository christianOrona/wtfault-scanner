//! Serve the problem-report endpoint.
//!
//!     aim-report-endpoint --dir /var/lib/wtfault-reports --port 8790
//!
//! Put it behind a TLS-terminating proxy; pass `--trust-forwarded-for` only
//! then. See `docs/REPORTS.md`.

use aim_report_endpoint::{router, Config};
use clap::Parser;
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(
    name = "aim-report-endpoint",
    about = "Receives WTFault Scanner problem reports",
    version
)]
struct Args {
    /// Folder reports are written to, one file each.
    #[arg(long)]
    dir: std::path::PathBuf,
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1")]
    bind: std::net::IpAddr,
    /// TCP port to listen on.
    #[arg(long, default_value_t = 8790)]
    port: u16,
    /// Reports accepted from one address per hour.
    #[arg(long, default_value_t = 5)]
    per_address: u32,
    /// Reports accepted from everybody per day.
    #[arg(long, default_value_t = 1000)]
    per_day: u32,
    /// Take the client address from X-Forwarded-For (only behind a proxy).
    #[arg(long)]
    trust_forwarded_for: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    std::fs::create_dir_all(&args.dir)?;
    let config = Config {
        dir: args.dir,
        per_address: args.per_address,
        window: Duration::from_secs(3600),
        per_day: args.per_day,
        trust_forwarded_for: args.trust_forwarded_for,
    };
    let addr = SocketAddr::new(args.bind, args.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening for problem reports");
    axum::serve(listener, router(config).into_make_service_with_connect_info::<SocketAddr>())
        .await?;
    Ok(())
}
