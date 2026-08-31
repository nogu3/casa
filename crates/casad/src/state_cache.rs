//! 属性最終値キャッシュ（issue #3: off 限定の no-op スキップ用）。
//!
//! 常駐 `mat listen` ストリームが受けた OnOff/on-off イベント（priming 含む —
//! 現在値の再配達はキャッシュにとってはご馳走）から、(node_id, endpoint) ごとの
//! 消灯/点灯の最終観測を覚える。ルール発火時、アクションが Matter の `off` で
//! 対象が「新鮮に消灯済み」と分かる場合だけコマンド実行をスキップする。
//!
//! `on` のスキップには**絶対に使わない**（NL68 の state=on 物理消灯固着。
//! 報告状態を信じて on を省くと点かない事故になる。off 側のみ安全）。
//! 鮮度 TTL は listen 切断でキャッシュがステイル化したときの「消えない事故」の保険。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::mat;

/// (node_id, endpoint) → OnOff/on-off の最終観測値と受信時刻。
pub struct StateCache {
    inner: Mutex<HashMap<(u64, u64), (bool, Instant)>>,
}

impl StateCache {
    pub fn new() -> Self {
        StateCache {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// mat listen イベント 1 件を取り込む。OnOff クラスタの on-off（bool 値）
    /// 以外は無視する。priming / recovered も取り込む（現在値の知識として有効）。
    pub fn record(&self, ev: &mat::Event) {
        self.record_at(ev, Instant::now());
    }

    fn record_at(&self, ev: &mat::Event, at: Instant) {
        if !is_onoff(&ev.cluster) || !is_on_off_attr(&ev.attribute) {
            return;
        }
        let serde_json::Value::Bool(v) = ev.value else {
            return;
        };
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        map.insert((ev.node_id, ev.endpoint), (v, at));
    }

    /// (node, endpoint) の最終観測が「消灯 (false)」かつ受信から `ttl` 以内なら true。
    /// 観測なし・点灯・ステイルは false（= スキップしない）。
    pub fn is_off_fresh(&self, node: u64, endpoint: u64, ttl: Duration) -> bool {
        let map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(&(node, endpoint)) {
            Some((false, at)) => at.elapsed() <= ttl,
            _ => false,
        }
    }
}

/// cluster が OnOff（chip-tool 名 "onoff" / 数値 6）か。
fn is_onoff(cluster: &serde_json::Value) -> bool {
    match cluster {
        serde_json::Value::String(s) => s.eq_ignore_ascii_case("onoff"),
        serde_json::Value::Number(n) => n.as_u64() == Some(0x0006),
        _ => false,
    }
}

/// attribute が on-off（chip-tool 名 "on-off" / 数値 0）か。
fn is_on_off_attr(attr: &serde_json::Value) -> bool {
    match attr {
        serde_json::Value::String(s) => s.eq_ignore_ascii_case("on-off"),
        serde_json::Value::Number(n) => n.as_u64() == Some(0x0000),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn onoff_event(node: u64, ep: u64, value: serde_json::Value) -> mat::Event {
        mat::Event {
            node_id: node,
            endpoint: ep,
            cluster: serde_json::json!("onoff"),
            attribute: serde_json::json!("on-off"),
            value,
            priming: false,
            recovered: false,
        }
    }

    const TTL: Duration = Duration::from_secs(600);

    #[test]
    fn off_event_makes_target_skippable() {
        let c = StateCache::new();
        c.record(&onoff_event(24, 1, serde_json::json!(false)));
        assert!(c.is_off_fresh(24, 1, TTL));
    }

    #[test]
    fn on_event_or_unknown_target_is_not_skippable() {
        let c = StateCache::new();
        c.record(&onoff_event(24, 1, serde_json::json!(true)));
        assert!(!c.is_off_fresh(24, 1, TTL)); // 点灯中
        assert!(!c.is_off_fresh(23, 1, TTL)); // 観測なし
        assert!(!c.is_off_fresh(24, 2, TTL)); // endpoint 違い
    }

    #[test]
    fn stale_entry_is_not_skippable() {
        let c = StateCache::new();
        let past = Instant::now() - Duration::from_secs(601);
        c.record_at(&onoff_event(24, 1, serde_json::json!(false)), past);
        assert!(!c.is_off_fresh(24, 1, TTL));
    }

    #[test]
    fn newer_observation_overwrites_older() {
        let c = StateCache::new();
        c.record(&onoff_event(24, 1, serde_json::json!(false)));
        c.record(&onoff_event(24, 1, serde_json::json!(true)));
        assert!(!c.is_off_fresh(24, 1, TTL));
    }

    #[test]
    fn priming_and_numeric_ids_are_recorded() {
        let c = StateCache::new();
        // priming の現在値再配達もキャッシュには有効。
        let mut ev = onoff_event(24, 1, serde_json::json!(false));
        ev.priming = true;
        c.record(&ev);
        assert!(c.is_off_fresh(24, 1, TTL));
        // 未知 ID の数値形（cluster 6 / attribute 0）も OnOff として拾う。
        let ev = mat::Event {
            cluster: serde_json::json!(6),
            attribute: serde_json::json!(0),
            ..onoff_event(30, 1, serde_json::json!(false))
        };
        c.record(&ev);
        assert!(c.is_off_fresh(30, 1, TTL));
    }

    #[test]
    fn non_onoff_events_are_ignored() {
        let c = StateCache::new();
        let ev = mat::Event {
            cluster: serde_json::json!("occupancysensing"),
            attribute: serde_json::json!("occupancy"),
            ..onoff_event(24, 1, serde_json::json!(0))
        };
        c.record(&ev);
        assert!(!c.is_off_fresh(24, 1, TTL));
        // OnOff でも bool 以外の値は無視（防御）。
        c.record(&onoff_event(24, 1, serde_json::json!(0)));
        assert!(!c.is_off_fresh(24, 1, TTL));
    }
}
