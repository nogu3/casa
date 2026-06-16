//! casad 統合テスト共通ヘルパ。casad バイナリを子プロセスとして起動する。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const DUMMY_CONFIG: &str = r#"
version = 1

[devices.living_aircon]
protocol = "echonet"
ip = "192.0.2.10"
eoj = "0x013001"
"#;

/// 一時ディレクトリにダミー設定を書き、そのパスを返す。
pub fn write_config(dir: &Path, text: &str) -> PathBuf {
    let path = dir.join("devices.toml");
    std::fs::write(&path, text).unwrap();
    path
}

/// tests/fixtures 配下のスクリプトの絶対パス。
pub fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// casad を指定引数・環境変数で起動する。
pub fn run_casad(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_casad"));
    // 親環境の CASA_* がテストに漏れないよう明示的に消す。
    cmd.env_remove("CASA_CONFIG")
        .env_remove("CASA_BIN")
        .env_remove("CASA_FAKE_EXIT");
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

/// stderr の最終行（casad のエラー JSON）をパースする。
pub fn stderr_error_json(output: &Output) -> serde_json::Value {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let last = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_else(|| panic!("stderr is empty"));
    serde_json::from_str(last)
        .unwrap_or_else(|e| panic!("stderr last line is not valid JSON: {e}\nstderr: {stderr}"))
}
