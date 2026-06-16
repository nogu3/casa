mod action;
mod casa_runner;
mod cli;
mod engine;
mod rules;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use casa_core::config;
use casa_core::error::CasaError;

use crate::cli::{Cli, Command};

fn main() {
    // casa と同じく、診断ログは stderr に構造化（JSON）で出す。レベルは RUST_LOG で制御。
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    match run(Cli::parse()) {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("{}", err.to_stderr_json());
            std::process::exit(err.exit_code());
        }
    }
}

fn run(cli: Cli) -> Result<i32, CasaError> {
    match cli.command {
        Command::Exec { action, name } => {
            // link 側: 設定ロードと名前解決は casa-core で型安全に。未定義名は casa を
            // 起動する前に exit 11 で弾く（ルールエンジンが発火前にルールを検証できる根拠）。
            let config = config::load(cli.config.as_deref())?;
            config.device(&name)?;

            // spawn 側: 実機アクションは casa を子プロセスとして起動し、exit code を伝播する。
            let args = action.casa_args(&name, cli.config.as_deref());
            casa_runner::run_casa(&args)
        }
        Command::Check { rules } => {
            // 設定とルールを読み、参照デバイス存在と時刻書式を検証する。
            let config = config::load(cli.config.as_deref())?;
            let rule_file = rules::load(&rules)?;
            rule_file.validate(&config)?;
            engine::validate_schedule(&rule_file)?;

            // 検証結果のサマリを stdout に JSON で出す。casad が解釈したルールを
            // そのまま返すことで、UI / LLM が往復（書いた内容の解釈）を確認できる。
            let summary = serde_json::json!({
                "ok": true,
                "count": rule_file.rules.len(),
                "rules": rule_file.rules,
            });
            println!("{summary}");
            Ok(0)
        }
        Command::Run { rules, once, now } => {
            // run も check と同じ検証を通してから起動する（不正ルールで常駐させない）。
            let config = config::load(cli.config.as_deref())?;
            let rule_file = rules::load(&rules)?;
            rule_file.validate(&config)?;
            engine::validate_schedule(&rule_file)?;

            let now = now.map(|s| engine::parse_hm(&s)).transpose()?;
            engine::run(&rule_file, cli.config.as_deref(), engine::RunOpts { once, now })
        }
    }
}
