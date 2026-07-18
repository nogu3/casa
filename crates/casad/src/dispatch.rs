//! デバイス別ワーカー。ルールアクションの実行を listen / tick ループから切り離す。
//!
//! - `dispatch` は mpsc への送信のみで即戻る（listen の取りこぼし窓を作らない）。
//! - 同一デバイス宛は 1 本のワーカーが FIFO で処理する（ON→OFF の逆順実行を
//!   構造的に排除する）。
//! - 異デバイス間はワーカーが別なので並列に走る。
//!
//! 実行関数 `run` はジェネリクスで注入する。本番は casa の spawn
//! （[`crate::engine`] の run_one）、テストは記録クロージャを渡す。
//! ワーカー数は起動時の rules.toml から固定（ホットリロードは無い）。

use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc::{self, Sender};
use std::thread::Scope;

use crate::rules::{Rule, RuleFile};

/// rules.toml の `then.device` の distinct 集合（BTreeSet で順序決定的）。
pub fn distinct_devices(file: &RuleFile) -> BTreeSet<&str> {
    file.rules.iter().map(|r| r.then.device()).collect()
}

/// デバイス名 → ワーカーへの送信口。clone して複数ループ（event / time）で共有する。
#[derive(Clone)]
pub struct Dispatcher<'env> {
    senders: HashMap<String, Sender<&'env Rule>>,
}

impl<'env> Dispatcher<'env> {
    /// デバイスごとにチャネルとワーカースレッドを `scope` 内に張る。
    /// ワーカーは全 Sender が drop されるとキューを掃いて終了する
    /// （常駐では到達しない。テストではこれが「全アクション完了」の同期点になる）。
    pub fn new<'scope, F>(
        scope: &'scope Scope<'scope, 'env>,
        devices: impl IntoIterator<Item = &'env str>,
        run: F,
    ) -> Self
    where
        F: Fn(&'env Rule) + Send + Copy + 'scope,
    {
        let mut senders = HashMap::new();
        for device in devices {
            let (tx, rx) = mpsc::channel::<&'env Rule>();
            scope.spawn(move || {
                for rule in rx {
                    run(rule);
                }
            });
            senders.insert(device.to_string(), tx);
        }
        Dispatcher { senders }
    }

    /// ルールを対象デバイスのワーカーに積む。即戻る（実行完了は待たない）。
    /// 起動時に `then.device` 全件でワーカーを張るため、対応ワーカー無しは通常
    /// 到達しない防御分岐（warn して false）。
    pub fn dispatch(&self, rule: &'env Rule) -> bool {
        let device = rule.then.device();
        let Some(tx) = self.senders.get(device) else {
            tracing::warn!(rule = %rule.name, device, "no worker for device; dropping action");
            return false;
        };
        tracing::debug!(rule = %rule.name, device, "queueing rule action");
        tx.send(rule).is_ok()
    }

    /// 複数ルールを順に積み、積めた件数を返す。
    pub fn dispatch_all(&self, rules: Vec<&'env Rule>) -> usize {
        rules.into_iter().filter(|r| self.dispatch(r)).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use crate::rules::{Then, Trigger};

    /// テスト用ルールを直接組む（フィールドは pub）。when は使われないので固定でよい。
    fn rule(name: &str, device: &str) -> Rule {
        Rule {
            name: name.to_string(),
            when: Trigger::Time {
                at: "00:00".to_string(),
            },
            then: Then::On {
                device: device.to_string(),
            },
        }
    }

    #[test]
    fn distinct_devices_dedupes_then_targets() {
        let file = crate::rules::parse(
            r#"
version = 1
[[rules]]
name = "a"
when = { at = "07:00" }
then = { action = "on", device = "hallway_light" }
[[rules]]
name = "b"
when = { at = "22:00" }
then = { action = "off", device = "hallway_light" }
[[rules]]
name = "c"
when = { at = "23:00" }
then = { action = "off", device = "bedroom_light" }
"#,
        )
        .unwrap();
        let devices: Vec<&str> = distinct_devices(&file).into_iter().collect();
        assert_eq!(devices, ["bedroom_light", "hallway_light"]);
    }

    #[test]
    fn same_device_actions_run_in_fifo_order() {
        let slow = rule("slow_first", "dev_a");
        let fast = rule("second", "dev_a");
        let log = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], |r: &Rule| {
                // 先行ジョブを遅くしても FIFO が保たれる（並列なら second が先に完走する）。
                if r.name == "slow_first" {
                    std::thread::sleep(Duration::from_millis(100));
                }
                log.lock().unwrap().push(r.name.clone());
            });
            assert!(d.dispatch(&slow));
            assert!(d.dispatch(&fast));
            drop(d); // 全 Sender を落とす → ワーカーが掃いて終了 → scope が join
        });
        assert_eq!(*log.lock().unwrap(), ["slow_first", "second"]);
    }

    #[test]
    fn dispatch_returns_before_action_completes() {
        let r = rule("slow", "dev_a");
        let log = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], |r: &Rule| {
                std::thread::sleep(Duration::from_millis(300));
                log.lock().unwrap().push(r.name.clone());
            });
            let started = Instant::now();
            assert!(d.dispatch(&r));
            // 300ms のアクション完了を待っていないこと（余裕を見て 200ms 未満）。
            assert!(
                started.elapsed() < Duration::from_millis(200),
                "dispatch blocked on action"
            );
            drop(d);
        });
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[test]
    fn different_devices_run_in_parallel() {
        let slow = rule("slow", "dev_a");
        let fast = rule("fast", "dev_b");
        let done = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a", "dev_b"], |r: &Rule| {
                if r.then.device() == "dev_a" {
                    std::thread::sleep(Duration::from_millis(300));
                }
                done.lock().unwrap().push(r.name.clone());
            });
            // slow を先に積んでも、別デバイスの fast が先に完走する = 並列。
            assert!(d.dispatch(&slow));
            assert!(d.dispatch(&fast));
            drop(d);
        });
        assert_eq!(*done.lock().unwrap(), ["fast", "slow"]);
    }

    #[test]
    fn dispatch_to_unknown_device_returns_false() {
        let r = rule("ghost", "no_such_device");
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], |_r: &Rule| {});
            assert!(!d.dispatch(&r));
            drop(d);
        });
    }
}
