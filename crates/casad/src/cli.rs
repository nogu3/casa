//! casad の clap 定義。引数エラーは clap 既定の exit code 2。
//!
//! W1/W2 時点では雛形。提供するのは「アクション実行プリミティブ」`exec` と、
//! ルールファイルの「読込・検証」`check`。どちらも後段（W3: ルールエンジン）が
//! 内部で使う経路の最小形を CLI として露出したもの。

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::action::Action;

#[derive(Debug, Parser)]
#[command(
    name = "casad",
    version,
    about = "casa 上の常駐レイヤ（DSL ルールエンジン）。W2 時点では雛形。"
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
    /// ルールファイルをパースし、参照デバイスが設定に存在するか検証する。
    /// エンジンに載せる前にルールの正しさを確認する用途。
    Check {
        /// ルールファイル（rules.toml）のパス
        rules: PathBuf,
    },
}
