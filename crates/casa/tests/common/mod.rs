//! 統合テスト共通ヘルパ。casa バイナリを子プロセスとして起動する。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const DUMMY_CONFIG: &str = r#"
version = 1

[devices.living_aircon]
protocol = "echonet"
ip = "192.0.2.10"
eoj = "0x013001"

[devices.bedroom_light]
protocol = "echonet"
ip = "192.0.2.11"
eoj = "0x029101"

[devices.entry_lock]
protocol = "switchbot"
device_id = "DUMMY-XX-XX"
"#;

/// 一時ディレクトリにダミー設定を書き、そのパスを返す。
pub fn write_config(dir: &Path, text: &str) -> PathBuf {
    let path = dir.join("devices.toml");
    std::fs::write(&path, text).unwrap();
    path
}

/// casa を指定引数・環境変数で起動する。
pub fn run_casa(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_casa"));
    // 親環境の CASA_* がテストに漏れないよう明示的に消す。
    cmd.env_remove("CASA_CONFIG")
        .env_remove("CASA_ENL_BIN")
        .env_remove("CASA_MAT_BIN");
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

pub fn stdout_json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not valid JSON: {e}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// stderr の最終行（casa のエラー JSON）をパースする。
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
