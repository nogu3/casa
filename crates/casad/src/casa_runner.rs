//! casa バイナリ（実機アクションの実行役）を起動する境界。
//!
//! ハイブリッド構成の「spawn 側」。casad は設定ロードと名前解決を casa-core の link で
//! 型安全に行う一方、実機アクション（on/off/...）は casa を**子プロセスとして**起動する。
//! こうすることで:
//! - casa が落ちても casad のデバッグができる（実行時の影響範囲が閉じる）。
//! - 将来 casad を別言語で書き直すときも、保つべきは casa の CLI 境界だけで済む。
//!
//! アクション → casa 引数列の写像は [`crate::action::Action::casa_args`] が持つ。

use std::process::Command;

use casa_core::error::{CasaError, ErrorKind};

/// casa バイナリのパスを解決する。優先順: 環境変数 `CASA_BIN` > `PATH` 上の `casa`。
pub fn resolve_casa_bin() -> String {
    match std::env::var_os("CASA_BIN") {
        Some(p) if !p.is_empty() => p.to_string_lossy().into_owned(),
        _ => "casa".to_string(),
    }
}

/// casa を起動し、その exit code を伝播する。stdout/stderr は継承（透過）。
///
/// 起動失敗（バイナリ無し / 実行不可）は `child_not_found`（exit 12）。
pub fn run_casa(args: &[String]) -> Result<i32, CasaError> {
    let bin = resolve_casa_bin();
    tracing::debug!(bin, ?args, "spawning casa");

    let status = Command::new(&bin).args(args).status().map_err(|e| {
        CasaError::new(
            ErrorKind::ChildNotFound,
            format!("failed to execute casa \"{bin}\": {e}"),
        )
    })?;

    Ok(status.code().unwrap_or(1))
}
