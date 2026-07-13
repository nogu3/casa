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
    List {
        /// 各デバイスのプロパティマップを子 CLI から取得して含める
        /// （その場で取得するだけで、永続キャッシュはしない）
        #[arg(long)]
        describe: bool,
    },
    /// デバイスのプロパティマップ（introspection）を出力する
    Describe {
        /// 設定ファイル上のデバイス名
        name: String,
    },
    /// デバイスの電源を入れる（ECHONET Lite: EPC 0x80 = 0x30）
    On {
        /// 設定ファイル上のデバイス名またはグループ名
        name: String,
    },
    /// デバイスの電源を切る（ECHONET Lite: EPC 0x80 = 0x31）
    Off {
        /// 設定ファイル上のデバイス名またはグループ名
        name: String,
    },
    /// デバイスのプロパティを読む（ECHONET Lite: EPC 例 0x80 / Matter: endpoint/cluster/attribute 例 1/onoff/on-off）
    Get {
        /// 設定ファイル上のデバイス名
        name: String,
        /// プロパティ識別子。解釈はプロトコル依存（ECHONET: EPC `0x80` / Matter: `1/onoff/on-off`）
        property: String,
    },
    /// 設定ファイルを読んで妥当性を JSON で報告する（実機は呼ばない）。
    /// version・必須フィールド・未知プロトコルは読み込み時点で検証済み。
    /// 加えて、アダプタ未実装のプロトコル（実行時に protocol_unsupported になる）を
    /// warnings として可視化する。
    Validate,
    /// デバイスのプロパティに書き込む
    Set {
        /// 設定ファイル上のデバイス名またはグループ名
        name: String,
        /// プロパティ識別子。解釈はプロトコル依存（ECHONET: EPC `0x80` / Matter: `1/levelcontrol/current-level`）
        property: String,
        /// 書き込む値。子 CLI にそのまま渡す
        value: String,
    },
    /// プロトコル固有 CLI のコマンドを名前解決付きで呼び出す（長尾操作の汎用動詞）。
    /// `<command>` と後続引数は解釈せず子 CLI へそのまま渡す。
    /// casa 自身のフラグ（--config 等）は invoke より前に置くこと。
    Invoke {
        /// 設定ファイル上のデバイス名またはグループ名（グループは同一プロトコルのみ）
        name: String,
        /// 子 CLI のサブコマンド名（例: color-temp）。casa は解釈しない
        command: String,
        /// 子 CLI にそのまま渡す引数
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}
