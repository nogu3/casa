//! mat listen の起動と出力パース。casad の Matter イベント入力源。
//!
//! 購読状態は matd（常駐）が保持し、`mat listen` はその broadcast ストリームに
//! unix socket で繋いで 1 行 1 JSON を中継するだけの薄いクライアント。casad は
//! enl と同じ one-shot ループ（`--count 1` を繰り返し起動）で継続監視にする。
//! matd は自身の（再）購読時に現在状態を `priming: true` で再配達するため、
//! 誤発火防止のフィルタは casad 側（[`crate::engine`]）が担う。
//!
//! mat との結合は stdout JSON スキーマのみ。バイナリ解決は casa-core の runner を
//! 流用し、casa と同じ `CASA_MAT_BIN` / `[binaries]` 規約に従う。

use std::process::Command;

use serde::Deserialize;

use casa_core::error::{CasaError, ErrorKind};

/// `mat listen` の 1 イベント行。casad が使うフィールドのみ拾う。
#[derive(Debug, Deserialize)]
pub struct Event {
    pub node_id: u64,
    pub endpoint: u64,
    /// chip-tool 名（例 "occupancysensing"）または未知 ID の数値。突合には使わず
    /// debug ログ用に保持する。
    pub cluster: serde_json::Value,
    /// chip-tool 名（例 "occupancy"）または未知 ID の数値。
    pub attribute: serde_json::Value,
    pub value: serde_json::Value,
    /// matd（再）購読時の現在値再配達。状変ではないので発火してはならない。
    #[serde(default)]
    pub priming: bool,
}

/// `mat listen --count 1 --timeout-ms 0` を 1 回起動し、1 件以上のイベントを待って返す。
///
/// timeout 0 = 無期限（イベントが来るまでブロック）。呼ぶ側がループして継続監視にする。
/// フィルタは付けず全ノードを受け、突合は casad 側で行う（enl と同じ分担）。
pub fn listen_once(bin: &str) -> Result<Vec<Event>, CasaError> {
    tracing::debug!(bin, "spawning mat listen");

    let output = Command::new(bin)
        .args(["listen", "--count", "1", "--timeout-ms", "0"])
        .output()
        .map_err(|e| {
            CasaError::new(
                ErrorKind::ChildNotFound,
                format!("failed to execute mat \"{bin}\": {e}"),
            )
        })?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stderr.lines().filter(|l| !l.trim().is_empty()) {
        tracing::debug!(child = "mat", stderr = line, "mat listen stderr");
    }

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        return Err(CasaError::new(
            ErrorKind::ChildFailed(code),
            format!("mat listen exited with code {code}: {}", stderr.trim()),
        ));
    }

    let events = parse_lines(&output.stdout)?;
    // ルールに一致しないイベントも追えるよう、受信した全イベントを debug で残す。
    for ev in &events {
        tracing::debug!(
            node_id = ev.node_id,
            endpoint = ev.endpoint,
            cluster = %ev.cluster,
            attribute = %ev.attribute,
            value = %ev.value,
            priming = ev.priming,
            "event received"
        );
    }
    Ok(events)
}

/// stdout（1 行 1 JSON）を Event 列にパースする。空行は無視。
fn parse_lines(stdout: &[u8]) -> Result<Vec<Event>, CasaError> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|e| {
                CasaError::new(
                    ErrorKind::ChildInvalidOutput,
                    format!("mat listen stdout line is not valid JSON: {e}"),
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_event_line() {
        let line = br#"{"timestamp":"2026-07-23T00:00:00+09:00","node_id":16,"endpoint":1,"cluster":"occupancysensing","attribute":"occupancy","value":1,"priming":false}"#;
        let events = parse_lines(line).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].node_id, 16);
        assert_eq!(events[0].endpoint, 1);
        assert_eq!(events[0].attribute, serde_json::json!("occupancy"));
        assert_eq!(events[0].value, serde_json::json!(1));
        assert!(!events[0].priming);
    }

    #[test]
    fn parses_multiple_lines_and_skips_blank() {
        let lines = b"{\"node_id\":16,\"endpoint\":1,\"cluster\":1030,\"attribute\":0,\"value\":0,\"priming\":true}\n\n{\"node_id\":6,\"endpoint\":1,\"cluster\":\"onoff\",\"attribute\":\"onoff\",\"value\":true}\n";
        let events = parse_lines(lines).unwrap();
        assert_eq!(events.len(), 2);
        assert!(events[0].priming);
        // priming 欠落は false 既定。
        assert!(!events[1].priming);
        // 未知 ID は数値のまま。
        assert_eq!(events[0].cluster, serde_json::json!(1030));
    }

    #[test]
    fn rejects_invalid_json_line() {
        let err = parse_lines(b"not json\n").unwrap_err();
        assert_eq!(err.kind, ErrorKind::ChildInvalidOutput);
    }
}
