//! casad の clap 定義。引数エラーは clap 既定の exit code 2。
//!
//! W1 時点では雛形。提供するのは「アクション実行プリミティブ」`exec` のみで、
//! これは後段（W2: DSL、W3: ルールエンジン）が発火時に呼ぶ実行経路の最小形。

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "casad",
    version,
    about = "casa 上の常駐レイヤ（DSL ルールエンジン）。W1 時点では雛形。"
)]
pub struct Cli {
    /// 設定ファイルのパス（既定: $XDG_CONFIG_HOME/casa/devices.toml）。
    /// casa と同じ設定を共有し、解決したパスは casa へそのまま渡す。
    #[arg(long, global = true, env = "CASA_CONFIG", value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 名前を解決し、対応する casa アクションを実行する。
    /// ルールエンジンが発火時に使うアクション実行プリミティブの最小形。
    Exec {
        /// 実行するアクション
        #[arg(value_enum)]
        action: Action,
        /// 設定ファイル上のデバイス名
        name: String,
    },
}

/// casad が実行できるアクション。casa のサブコマンドへ写像される。
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "lower")]
pub enum Action {
    On,
    Off,
}

impl Action {
    /// 対応する casa サブコマンド名。
    pub fn subcommand(self) -> &'static str {
        match self {
            Action::On => "on",
            Action::Off => "off",
        }
    }
}
