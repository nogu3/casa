mod cli;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use casa_core::error::CasaError;
use casa_core::{config, ops, output};

use crate::cli::{Cli, Command};

fn main() {
    // 診断ログは stderr に構造化（JSON）で出す。レベルは RUST_LOG で制御。
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    if let Err(err) = run(cli) {
        // グループ部分失敗はメンバー別結果を stdout に出してから exit 15 する。
        if let Some(response) = &err.response {
            output::emit(response);
        }
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
            let groups = config
                .groups
                .iter()
                .map(|(name, group)| output::group_entry(name, group))
                .collect();
            output::list_response(devices, groups)
        }
        Command::Get { name, property } => ops::get(&config, &name, &property)?,
        Command::Set {
            name,
            property,
            value,
        } => ops::set(&config, &name, &property, &value)?,
        Command::Invoke {
            name,
            command,
            args,
        } => ops::invoke(&config, &name, &command, &args)?,
        Command::Validate => {
            let path = cli.config.clone().unwrap_or_else(config::default_path);
            ops::validate(&config, &path)
        }
        Command::ColorTemp {
            name,
            kelvin,
            mireds,
            transition,
        } => {
            let color = casa_core::adapter::ColorTemp {
                kelvin,
                mireds,
                transition,
            };
            ops::color_temp(&config, &name, &color)?
        }
        Command::Describe { name } => ops::describe(&config, &name)?,
        Command::On { name } => ops::power(&config, &name, true)?,
        Command::Off { name } => ops::power(&config, &name, false)?,
    };

    output::emit(&response);
    Ok(())
}
