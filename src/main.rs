mod adapter;
mod cli;
mod config;
mod error;
mod ops;
mod output;
mod runner;

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

    let response = match cli.command {
        Command::List { describe } => {
            let mut devices = Vec::with_capacity(config.devices.len());
            for (name, device) in &config.devices {
                let mut entry = output::device_entry(name, device);
                if describe {
                    // introspection 未対応のプロトコルは properties: null。
                    entry["properties"] =
                        ops::describe_device(&config, device)?.unwrap_or(serde_json::Value::Null);
                }
                devices.push(entry);
            }
            output::list_response(devices)
        }
        Command::Get { name, property } => ops::get(&config, &name, &property)?,
        Command::Set {
            name,
            property,
            value,
        } => ops::set(&config, &name, &property, &value)?,
        Command::Describe { name } => ops::describe(&config, &name)?,
        Command::On { name } => ops::power(&config, &name, true)?,
        Command::Off { name } => ops::power(&config, &name, false)?,
    };

    output::emit(&response);
    Ok(())
}
