mod cli;
mod config;
mod error;
mod output;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};
use crate::error::CasaError;

fn main() {
    // 診断ログは stderr に構造化（JSON）で出す。レベルは RUST_LOG で制御。
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    if let Err(err) = run(cli) {
        eprintln!("{}", err.to_stderr_json());
        std::process::exit(err.exit_code());
    }
}

fn run(cli: Cli) -> Result<(), CasaError> {
    let config = config::load(cli.config.as_deref())?;

    match cli.command {
        Command::List => {
            let devices = config
                .devices
                .iter()
                .map(|(name, device)| output::device_entry(name, device))
                .collect();
            output::emit(&output::list_response(devices));
        }
    }

    Ok(())
}
