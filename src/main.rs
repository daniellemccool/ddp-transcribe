//! The thin binary (0045). `lib.rs` is the crate's single module root, so this
//! file carries no `mod` line at all: it parses arguments, initializes tracing,
//! hands off to the library's `dispatch`, renders any error via anyhow's
//! `Termination` impl, and owns the one `std::process::exit` in the program.

use clap::Parser;
use ddp_transcribe::LogFormat;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = ddp_transcribe::Cli::parse();
    init_tracing(cli.log_format());
    let exit = ddp_transcribe::dispatch(cli).await?;
    std::process::exit(exit.code());
}

fn init_tracing(format: LogFormat) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // Logs go to stderr, never stdout: `status --json` (and any future
    // machine-readable command output) must be pure, parseable JSON on
    // stdout with no log lines interleaved. `fmt()`'s default writer is
    // stdout, so both branches route explicitly to stderr.
    match format {
        LogFormat::Human => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .init();
        }
        LogFormat::Json => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .init();
        }
    }
}
