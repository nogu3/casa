//! clap によるコマンドライン定義。引数エラーは clap 既定の exit code 2。

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "casa", version, about = "スマートホーム横断クライアント")]
pub struct Cli {
    /// 設定ファイルのパス（既定: $XDG_CONFIG_HOME/casa/devices.toml）
    #[arg(long, global = true, env = "CASA_CONFIG", value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 設定済みデバイスの一覧を JSON で出力する
    List,
}
