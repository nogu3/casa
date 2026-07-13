//! casad の clap 定義。引数エラーは clap 既定の exit code 2。
//!
//! W1/W2 時点では雛形。提供するのは「アクション実行プリミティブ」`exec` と、
//! ルールファイルの「読込・検証」`check`。どちらも後段（W3: ルールエンジン）が
//! 内部で使う経路の最小形を CLI として露出したもの。

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::rules::Then;

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
        #[command(subcommand)]
        action: ExecAction,
    },
    /// ルールファイルをパースし、参照デバイスが設定に存在するか・時刻が正しいか検証する。
    /// エンジンに載せる前にルールの正しさを確認する用途。
    Check {
        /// ルールファイル（rules.toml）のパス
        rules: PathBuf,
    },
    /// ルールエンジンを起動する。既定は常駐し、時刻トリガ（毎分の境界で評価）と
    /// イベントトリガ（enl listen を回して状変通知に反応）を並行に走らせる。
    Run {
        /// ルールファイル（rules.toml）のパス
        rules: PathBuf,
        /// 時刻トリガを 1 回だけ評価して終了する（cron 毎分起動、またはデバッグ用）。
        #[arg(long)]
        once: bool,
        /// 現在時刻を HH:MM で上書きする（`--once` 併用のデバッグ用）。
        #[arg(long, value_name = "HH:MM", requires = "once")]
        now: Option<String>,
        /// enl listen を 1 回だけ起動し、得た通知でイベントトリガを評価して終了する
        /// （デバッグ用。通知が来るまでブロックする）。
        #[arg(long, conflicts_with = "once")]
        listen_once: bool,
    },
}

/// `casad exec` のアクション。rules の `then` と同じ語彙（on / off / invoke）。
#[derive(Debug, Subcommand)]
pub enum ExecAction {
    /// デバイス（またはグループ）の電源を入れる
    On {
        /// 設定ファイル上のデバイス名またはグループ名
        name: String,
    },
    /// デバイス（またはグループ）の電源を切る
    Off {
        /// 設定ファイル上のデバイス名またはグループ名
        name: String,
    },
    /// 子 CLI のコマンドを名前解決付きで呼び出す（casa invoke へ委譲）。
    /// casad 自身のフラグ（--config 等）は exec より前に置くこと。
    Invoke {
        /// 設定ファイル上のデバイス名またはグループ名
        name: String,
        /// 子 CLI のサブコマンド名（例: color-temp）
        command: String,
        /// 子 CLI にそのまま渡す引数
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

impl ExecAction {
    /// rules の `Then` へ変換する。exec と rules で casa 引数列の写像を共有する。
    pub fn into_then(self) -> Then {
        match self {
            ExecAction::On { name } => Then::On { device: name },
            ExecAction::Off { name } => Then::Off { device: name },
            ExecAction::Invoke {
                name,
                command,
                args,
            } => Then::Invoke {
                device: name,
                command,
                args,
            },
        }
    }
}
