//! enl listen の起動と出力パース。casad のイベント入力源。
//!
//! enl 自身の設計（コメント）が「状変連動は listen を外部ループから回して組む」と
//! 明言しており、その「外部ループ」が casad。enl は one-shot（count 件で終了）なので、
//! casad が繰り返し起動して継続監視にする。
//!
//! enl との結合は stdout JSON スキーマのみ（`enl schema listen` が安定契約）。crate
//! 依存はしない。enl バイナリの解決は casa-core の runner を流用し、casa と同じ
//! `CASA_ENL_BIN` / `[binaries]` 規約に従う。

use std::process::Command;

use serde::Deserialize;

use casa_core::error::{CasaError, ErrorKind};

/// `enl listen` の出力（`{ "events": [...] }`）。casad が使うフィールドのみ拾う。
#[derive(Debug, Deserialize)]
struct ListenOutput {
    #[serde(default)]
    events: Vec<Event>,
}

/// 1 件の INF / INFC 通知。
#[derive(Debug, Deserialize)]
pub struct Event {
    /// 通知の送信元 IP。
    pub ip: String,
    /// 通知元 EOJ（6 hex 桁, 例 "013001"）。
    pub seoj: String,
    #[serde(default)]
    pub properties: Vec<Prop>,
}

/// 通知に含まれるプロパティ。
#[derive(Debug, Deserialize)]
pub struct Prop {
    /// EPC（2 hex 桁, 大文字, 例 "80"）。
    pub epc: String,
    /// EDT の生 hex（常に存在しロスレス, 例 "30"）。
    pub edt_hex: String,
}

/// `enl listen --count 1 --timeout-ms 0` を 1 回起動し、1 件以上の通知を待って返す。
///
/// timeout 0 = 無期限（通知が来るまでブロック）。これを呼ぶ側がループして継続監視にする。
/// フィルタは付けず全通知を受け、突合は casad 側（[`crate::engine`]）で行う。
pub fn listen_once(bin: &str) -> Result<Vec<Event>, CasaError> {
    tracing::debug!(bin, "spawning enl listen");

    let output = Command::new(bin)
        .args(["listen", "--count", "1", "--timeout-ms", "0"])
        .output()
        .map_err(|e| {
            CasaError::new(
                ErrorKind::ChildNotFound,
                format!("failed to execute enl \"{bin}\": {e}"),
            )
        })?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stderr.lines().filter(|l| !l.trim().is_empty()) {
        tracing::debug!(child = "enl", stderr = line, "enl listen stderr");
    }

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        return Err(CasaError::new(
            ErrorKind::ChildFailed(code),
            format!("enl listen exited with code {code}: {}", stderr.trim()),
        ));
    }

    let parsed: ListenOutput = serde_json::from_slice(&output.stdout).map_err(|e| {
        CasaError::new(
            ErrorKind::ChildInvalidOutput,
            format!("enl listen stdout is not valid JSON: {e}"),
        )
    })?;
    // ルールに一致しない通知も追えるよう、受信した全イベントを debug で残す。
    for ev in &parsed.events {
        for prop in &ev.properties {
            tracing::debug!(ip = %ev.ip, seoj = %ev.seoj, epc = %prop.epc, edt = %prop.edt_hex, "event received");
        }
    }
    Ok(parsed.events)
}
