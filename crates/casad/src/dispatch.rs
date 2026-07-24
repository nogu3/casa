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

use crate::rules::{Rule, RuleFile, Then};

/// rules.toml の全 `then` アクションの対象名の distinct 集合（BTreeSet で順序決定的）。
pub fn distinct_devices(file: &RuleFile) -> BTreeSet<&str> {
    file.rules
        .iter()
        .flat_map(|r| r.then.actions())
        .map(|t| t.device())
        .collect()
}

/// ワーカーに積む仕事の単位。ルールは発火ログ用に持ち回る。
#[derive(Clone, Copy)]
pub struct Job<'env> {
    pub rule: &'env Rule,
    pub then: &'env Then,
}

/// デバイス名 → ワーカーへの送信口。clone して複数ループ（event / time）で共有する。
#[derive(Clone)]
pub struct Dispatcher<'env> {
    senders: HashMap<String, Sender<Job<'env>>>,
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
        F: Fn(Job<'env>) + Send + Copy + 'scope,
    {
        let mut senders = HashMap::new();
        for device in devices {
            let (tx, rx) = mpsc::channel::<Job<'env>>();
            scope.spawn(move || {
                for job in rx {
                    run(job);
                }
            });
            senders.insert(device.to_string(), tx);
        }
        Dispatcher { senders }
    }

    /// 1 アクションを対象デバイスのワーカーに積む。即戻る（実行完了は待たない）。
    /// 起動時に全アクションの対象でワーカーを張るため、対応ワーカー無しは通常
    /// 到達しない防御分岐（warn して false）。
    fn dispatch_job(&self, job: Job<'env>) -> bool {
        let device = job.then.device();
        let Some(tx) = self.senders.get(device) else {
            tracing::warn!(rule = %job.rule.name, device, "no worker for device; dropping action");
            return false;
        };
        tracing::debug!(rule = %job.rule.name, device, "queueing rule action");
        match tx.send(job) {
            Ok(()) => true,
            Err(_) => {
                // ワーカー消失（panic 等）。常駐では scope join に到達せず気づけないため
                // ここで可視化する。
                tracing::warn!(rule = %job.rule.name, device, "worker gone; dropping action");
                false
            }
        }
    }

    /// ルールの全アクションを**記載順**に積む。積めた件数を返す。
    /// 同一デバイス宛は同じチャネルに記載順で入るため、宣言順の実行が保証される。
    pub fn dispatch(&self, rule: &'env Rule) -> usize {
        rule.then
            .actions()
            .iter()
            .filter(|&then| self.dispatch_job(Job { rule, then }))
            .count()
    }

    /// 複数ルールを順に積み、積めたアクション件数の合計を返す。
    pub fn dispatch_all(&self, rules: Vec<&'env Rule>) -> usize {
        rules.into_iter().map(|r| self.dispatch(r)).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use crate::rules::{Then, Thens, Trigger};

    /// テスト用ルールを直接組む（フィールドは pub）。when は使われないので固定でよい。
    fn rule(name: &str, device: &str) -> Rule {
        Rule {
            name: name.to_string(),
            when: Trigger::Time {
                at: "00:00".to_string(),
            },
            then: Thens::One(Then::On {
                device: device.to_string(),
            }),
        }
    }

    /// 複数アクションを持つテスト用ルール。when は使われないので固定でよい。
    fn multi_rule(name: &str, devices: &[&str]) -> Rule {
        Rule {
            name: name.to_string(),
            when: Trigger::Time {
                at: "00:00".to_string(),
            },
            then: Thens::Many(
                devices
                    .iter()
                    .map(|d| Then::On {
                        device: d.to_string(),
                    })
                    .collect(),
            ),
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
    fn distinct_devices_collects_every_then_action() {
        let file = crate::rules::parse(
            r#"
version = 1
[[rules]]
name = "a"
when = { at = "07:00" }
then = [
  { action = "on", device = "desk_light" },
  { action = "on", device = "desk_tape_light" },
]
[[rules]]
name = "b"
when = { at = "22:00" }
then = { action = "off", device = "desk_light" }
"#,
        )
        .unwrap();
        let devices: Vec<&str> = distinct_devices(&file).into_iter().collect();
        assert_eq!(devices, ["desk_light", "desk_tape_light"]);
    }

    #[test]
    fn multi_action_rule_fans_out_to_each_target_worker() {
        let r = multi_rule("fanout", &["dev_a", "dev_b"]);
        let seen = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a", "dev_b"], |j: Job| {
                seen.lock().unwrap().push(j.then.device().to_string());
            });
            assert_eq!(d.dispatch(&r), 2);
            drop(d);
        });
        let mut got = seen.lock().unwrap().clone();
        got.sort();
        assert_eq!(got, ["dev_a", "dev_b"]);
    }

    #[test]
    fn same_device_actions_of_one_rule_run_in_declaration_order() {
        // 同一デバイス宛の 2 アクションは同じワーカーに記載順で入る。
        // 先行アクションを遅くしても順序が保たれる（並列なら second が先に完走する）。
        let r = Rule {
            name: "ordered".to_string(),
            when: Trigger::Time {
                at: "00:00".to_string(),
            },
            then: Thens::Many(vec![
                Then::On {
                    device: "dev_a".to_string(),
                },
                Then::Invoke {
                    device: "dev_a".to_string(),
                    command: "color-temp".to_string(),
                    args: vec![],
                },
            ]),
        };
        let log = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], |j: Job| {
                if matches!(j.then, Then::On { .. }) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                log.lock().unwrap().push(match j.then {
                    Then::On { .. } => "on",
                    Then::Invoke { .. } => "invoke",
                    Then::Off { .. } => "off",
                });
            });
            assert_eq!(d.dispatch(&r), 2);
            drop(d);
        });
        assert_eq!(*log.lock().unwrap(), ["on", "invoke"]);
    }

    #[test]
    fn different_device_actions_of_one_rule_run_in_parallel() {
        // 遅い dev_a を先に積んでも、別ワーカーの dev_b が先に完走する = 並列。
        let r = multi_rule("parallel", &["dev_a", "dev_b"]);
        let done = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a", "dev_b"], |j: Job| {
                if j.then.device() == "dev_a" {
                    std::thread::sleep(Duration::from_millis(300));
                }
                done.lock().unwrap().push(j.then.device().to_string());
            });
            assert_eq!(d.dispatch(&r), 2);
            drop(d);
        });
        assert_eq!(*done.lock().unwrap(), ["dev_b", "dev_a"]);
    }

    #[test]
    fn dispatch_counts_only_actions_with_a_worker() {
        // ワーカーの無い対象は積めない（防御分岐）。積めた件数だけ数える。
        let r = multi_rule("partial", &["dev_a", "no_such_device"]);
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], |_j: Job| {});
            assert_eq!(d.dispatch(&r), 1);
            drop(d);
        });
    }

    #[test]
    fn same_device_actions_run_in_fifo_order() {
        let slow = rule("slow_first", "dev_a");
        let fast = rule("second", "dev_a");
        let log = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], |j: Job| {
                // 先行ジョブを遅くしても FIFO が保たれる（並列なら second が先に完走する）。
                if j.rule.name == "slow_first" {
                    std::thread::sleep(Duration::from_millis(100));
                }
                log.lock().unwrap().push(j.rule.name.clone());
            });
            assert_eq!(d.dispatch(&slow), 1);
            assert_eq!(d.dispatch(&fast), 1);
            drop(d); // 全 Sender を落とす → ワーカーが掃いて終了 → scope が join
        });
        assert_eq!(*log.lock().unwrap(), ["slow_first", "second"]);
    }

    #[test]
    fn dispatch_returns_before_action_completes() {
        let r = rule("slow", "dev_a");
        let log = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], |j: Job| {
                std::thread::sleep(Duration::from_millis(300));
                log.lock().unwrap().push(j.rule.name.clone());
            });
            let started = Instant::now();
            assert_eq!(d.dispatch(&r), 1);
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
            let d = Dispatcher::new(s, ["dev_a", "dev_b"], |j: Job| {
                if j.then.device() == "dev_a" {
                    std::thread::sleep(Duration::from_millis(300));
                }
                done.lock().unwrap().push(j.rule.name.clone());
            });
            // slow を先に積んでも、別デバイスの fast が先に完走する = 並列。
            assert_eq!(d.dispatch(&slow), 1);
            assert_eq!(d.dispatch(&fast), 1);
            drop(d);
        });
        assert_eq!(*done.lock().unwrap(), ["fast", "slow"]);
    }

    #[test]
    fn dispatch_to_unknown_device_counts_zero() {
        let r = rule("ghost", "no_such_device");
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], |_j: Job| {});
            assert_eq!(d.dispatch(&r), 0);
            drop(d);
        });
    }
}
