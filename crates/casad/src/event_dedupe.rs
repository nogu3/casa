//! Matter デバイスイベントの重複配達デデュープ（EventNumber の単調性）。
//!
//! Matter の EventNumber はノードごとに単調増加する。常駐 `mat listen` ストリームが
//! 同じイベントを二度届けた場合（matd 再購読の再配達など）に二重発火しないよう、
//! (node_id, endpoint, cluster) ごとに最大 event_number を覚えて、それ以下の番号を落とす。
//! キーを cluster 単位にしているのは、matd が異クラスタ間で順序を入れ替えて配っても
//! 取りこぼさないため。
//!
//! 状態は in-memory のみ（casad 再起動で消える）。ストリームスレッドだけが触るので
//! 同期は不要。

use std::collections::HashMap;

use crate::mat::MatterEvent;

/// (node_id, endpoint, cluster の文字列表現) → これまでに見た最大 event_number。
#[derive(Debug, Default)]
pub struct EventWatermark {
    seen: HashMap<(u64, u64, String), u64>,
}

impl EventWatermark {
    pub fn new() -> Self {
        Self::default()
    }

    /// この行を通してよいか。
    /// - `event_number` 無し → 常に通す（watermark は触らない）。
    /// - `start-up`（BasicInformation 0x00）→ その node のキーを全部クリアしてから自身の
    ///   番号を置いて通す（工場リセット等でカウンタが巻き戻った node を復帰させる）。
    /// - 番号が watermark 以下 → 重複として落とす。
    /// - それ以外 → watermark を更新して通す。
    ///
    /// priming 行も通して watermark を更新させる（再配達の最新番号を覚えるため）。
    /// priming を発火させない判定は engine 側の責務。
    pub fn accept(&mut self, ev: &MatterEvent) -> bool {
        let Some(n) = ev.event_number else {
            return true;
        };
        let key = key_of(ev);
        if is_start_up(&ev.event) {
            self.seen.retain(|(node, _, _), _| *node != ev.node_id);
            self.seen.insert(key, n);
            return true;
        }
        match self.seen.get(&key) {
            Some(&max) if n <= max => false,
            _ => {
                self.seen.insert(key, n);
                true
            }
        }
    }

    /// ログ用: この行のキーに対する現在の watermark。
    pub fn watermark_of(&self, ev: &MatterEvent) -> Option<u64> {
        self.seen.get(&key_of(ev)).copied()
    }
}

fn key_of(ev: &MatterEvent) -> (u64, u64, String) {
    (ev.node_id, ev.endpoint, ev.cluster.to_string())
}

/// event が BasicInformation の StartUp（chip-tool 名 "start-up" / 数値 0）か。
fn is_start_up(event: &serde_json::Value) -> bool {
    match event {
        serde_json::Value::String(s) => s.eq_ignore_ascii_case("start-up"),
        serde_json::Value::Number(n) => n.as_u64() == Some(0),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(node: u64, ep: u64, cluster: &str, event: &str, n: Option<u64>) -> MatterEvent {
        MatterEvent {
            node_id: node,
            endpoint: ep,
            cluster: serde_json::json!(cluster),
            event: serde_json::json!(event),
            event_number: n,
            data: None,
            priming: false,
        }
    }

    #[test]
    fn accepts_increasing_numbers_and_drops_repeats_and_older() {
        let mut w = EventWatermark::new();
        assert!(w.accept(&ev(25, 2, "switch", "initial-press", Some(10))));
        assert!(w.accept(&ev(25, 2, "switch", "multi-press-complete", Some(11))));
        // 同じ番号の再配達。
        assert!(!w.accept(&ev(25, 2, "switch", "multi-press-complete", Some(11))));
        // 古い番号。
        assert!(!w.accept(&ev(25, 2, "switch", "initial-press", Some(10))));
        assert_eq!(w.watermark_of(&ev(25, 2, "switch", "x", None)), Some(11));
        // 次の新しい番号は通る。
        assert!(w.accept(&ev(25, 2, "switch", "initial-press", Some(12))));
    }

    #[test]
    fn keys_are_independent_per_node_endpoint_cluster() {
        let mut w = EventWatermark::new();
        assert!(w.accept(&ev(25, 2, "switch", "initial-press", Some(100))));
        // 別 cluster（同 node）: 小さい番号でも通る。
        assert!(w.accept(&ev(25, 1, "booleanstate", "state-change", Some(5))));
        // 別 endpoint。
        assert!(w.accept(&ev(25, 3, "switch", "initial-press", Some(5))));
        // 別 node。
        assert!(w.accept(&ev(19, 2, "switch", "initial-press", Some(5))));
        // 元のキーの watermark は保たれている。
        assert!(!w.accept(&ev(25, 2, "switch", "initial-press", Some(100))));
    }

    #[test]
    fn missing_event_number_always_passes_without_touching_watermark() {
        let mut w = EventWatermark::new();
        assert!(w.accept(&ev(25, 2, "switch", "initial-press", Some(10))));
        assert!(w.accept(&ev(25, 2, "switch", "initial-press", None)));
        assert!(w.accept(&ev(25, 2, "switch", "initial-press", None)));
        assert_eq!(w.watermark_of(&ev(25, 2, "switch", "x", None)), Some(10));
    }

    #[test]
    fn priming_updates_watermark_so_live_repeat_is_dropped() {
        let mut w = EventWatermark::new();
        let mut primed = ev(19, 1, "booleanstate", "state-change", Some(590151));
        primed.priming = true;
        assert!(w.accept(&primed));
        // 同じ番号が priming 無しで再配達されても落ちる。
        assert!(!w.accept(&ev(19, 1, "booleanstate", "state-change", Some(590151))));
        assert!(w.accept(&ev(19, 1, "booleanstate", "state-change", Some(590152))));
    }

    #[test]
    fn start_up_resets_every_key_of_that_node_only() {
        let mut w = EventWatermark::new();
        assert!(w.accept(&ev(19, 1, "booleanstate", "state-change", Some(590151))));
        assert!(w.accept(&ev(19, 2, "switch", "initial-press", Some(590160))));
        assert!(w.accept(&ev(25, 2, "switch", "initial-press", Some(700))));
        // node 19 が工場リセットされて番号 1 から始まった。
        assert!(w.accept(&ev(19, 0, "basicinformation", "start-up", Some(1))));
        // node 19 の他クラスタも小さい番号で通る。
        assert!(w.accept(&ev(19, 1, "booleanstate", "state-change", Some(2))));
        assert!(w.accept(&ev(19, 2, "switch", "initial-press", Some(3))));
        // start-up 自身の番号は watermark になる。
        assert!(!w.accept(&ev(19, 0, "basicinformation", "shut-down", Some(1))));
        // node 25 は影響を受けない。
        assert!(!w.accept(&ev(25, 2, "switch", "initial-press", Some(700))));
    }

    #[test]
    fn start_up_is_recognized_case_insensitively_and_by_numeric_id() {
        let mut w = EventWatermark::new();
        assert!(w.accept(&ev(19, 1, "booleanstate", "state-change", Some(100))));
        assert!(w.accept(&ev(19, 0, "basicinformation", "Start-Up", Some(1))));
        assert!(w.accept(&ev(19, 1, "booleanstate", "state-change", Some(2))));
        // 未知 ID 形: BasicInformation の StartUp は event ID 0。
        let mut numeric = ev(19, 0, "basicinformation", "start-up", Some(1));
        numeric.event = serde_json::json!(0);
        assert!(w.accept(&numeric));
        assert!(w.accept(&ev(19, 1, "booleanstate", "state-change", Some(1))));
    }
}
