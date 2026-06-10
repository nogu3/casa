//! 子 CLI ランナー。バイナリ名と引数を受け取って起動し、stdout の JSON を返す。
//!
//! casa はプロトコルを直接喋らない。実機との通信はすべてここから起動する
//! 兄弟 CLI（`enl` 等）に委譲する。

use std::process::Command;

use crate::config::Config;
use crate::error::{CasaError, ErrorKind};

/// 子 CLI バイナリのパスを解決する。
/// 優先順: 環境変数 `CASA_<BIN>_BIN` > 設定ファイル `[binaries]` > `PATH` 上の名前。
pub fn resolve_bin(name: &str, config: &Config) -> String {
    let env_key = format!("CASA_{}_BIN", name.to_uppercase());
    if let Some(path) = std::env::var_os(&env_key) {
        if !path.is_empty() {
            return path.to_string_lossy().into_owned();
        }
    }
    if let Some(path) = config.binaries.get(name) {
        return path.clone();
    }
    name.to_string()
}

/// 子 CLI を起動し、stdout を JSON としてパースして返す。
///
/// - 起動失敗（バイナリ無し / 実行不可）→ `child_not_found`（exit 12）
/// - 非ゼロ終了 → `child_failed`（子の exit code をそのまま伝播）
/// - stdout が JSON でない → `child_invalid_output`（exit 13）
/// - 子の stderr は呑まず、debug レベルで casa の stderr に転送する
pub fn run(bin: &str, args: &[String]) -> Result<serde_json::Value, CasaError> {
    tracing::debug!(bin, ?args, "spawning child CLI");

    let output = Command::new(bin).args(args).output().map_err(|e| {
        CasaError::new(
            ErrorKind::ChildNotFound,
            format!("failed to execute child CLI \"{bin}\": {e}"),
        )
    })?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stderr.lines().filter(|l| !l.trim().is_empty()) {
        tracing::debug!(child = bin, stderr = line, "child CLI stderr");
    }

    if !output.status.success() {
        // 子 CLI 由来のエラーは元の exit code を保ち、呼び出し側が
        // 「タイムアウトかリジェクトか」等を区別できるようにする。
        let code = output.status.code().unwrap_or(1);
        return Err(CasaError::new(
            ErrorKind::ChildFailed(code),
            format!("\"{bin}\" exited with code {code}: {}", stderr.trim()),
        ));
    }

    serde_json::from_slice(&output.stdout).map_err(|e| {
        CasaError::new(
            ErrorKind::ChildInvalidOutput,
            format!("\"{bin}\" stdout is not valid JSON: {e}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    fn config_with_binaries(toml_binaries: &str) -> Config {
        config::parse(&format!("version = 1\n[binaries]\n{toml_binaries}")).unwrap()
    }

    #[test]
    fn resolve_bin_defaults_to_name() {
        let config = config::parse("version = 1").unwrap();
        assert_eq!(resolve_bin("enl", &config), "enl");
    }

    #[test]
    fn resolve_bin_prefers_config_binaries() {
        let config = config_with_binaries("enl = \"/opt/bin/enl\"");
        assert_eq!(resolve_bin("enl", &config), "/opt/bin/enl");
    }
}
