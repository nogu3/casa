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
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::Scope;
use std::time::{Duration, Instant};

use crate::rules::{Rule, RuleFile, Settings, Then, Trigger};

/// デバイス別ワーカーの送信抑制ポリシー（issue #5: off→on 連射でのプラグ固着緩和）。
///
/// - `off_grace`: **イベント由来**の off を保留する猶予。保留中に on が来たら off は
///   破棄される（conflation）。時刻トリガの off には適用しない（取り消したいケースが
///   無く、消灯が遅れるだけ）。
/// - `min_gap`: 同一デバイスへの連続コマンド送信の最小間隔。
#[derive(Clone, Copy, Debug)]
pub struct WorkerPolicy {
    pub off_grace: Duration,
    pub min_gap: Duration,
}

impl WorkerPolicy {
    /// 抑制なし（従来挙動）。テストの土台用。
    #[cfg(test)]
    pub const ZERO: WorkerPolicy = WorkerPolicy {
        off_grace: Duration::ZERO,
        min_gap: Duration::ZERO,
    };

    pub fn from_settings(s: &Settings) -> Self {
        WorkerPolicy {
            off_grace: Duration::from_secs(s.off_grace_secs),
            min_gap: Duration::from_secs(s.min_gap_secs),
        }
    }
}

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
        policy: WorkerPolicy,
        run: F,
    ) -> Self
    where
        F: Fn(Job<'env>) + Send + Copy + 'scope,
    {
        let mut senders = HashMap::new();
        for device in devices {
            let (tx, rx) = mpsc::channel::<Job<'env>>();
            scope.spawn(move || worker_loop(rx, policy, run));
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

/// この job に off-grace を適用するか。イベント由来（人感などの連射源）の off のみ。
/// 時刻トリガの off は即時実行（保留しても取り消される見込みが無く、遅れるだけ）。
fn grace_applies(job: &Job<'_>, policy: WorkerPolicy) -> bool {
    !policy.off_grace.is_zero()
        && matches!(job.then, Then::Off { .. })
        && matches!(
            job.rule.when,
            Trigger::Event { .. } | Trigger::MatterEvent { .. }
        )
}

/// 1 デバイス分のワーカーループ。FIFO 実行に off-grace / conflation / min-gap を重ねる。
///
/// - イベント由来の off は `pending_off` に予約し、grace 満了まで送らない。
/// - on が来たら予約 off を破棄して on を実行（off→on 連射がデバイスに届かない）。
/// - 時刻トリガの off は即実行し、予約 off も破棄（off は送信済みになるので冗長）。
/// - invoke は予約に触らず実行（直交・v1）。
/// - チャネル切断（シャットダウン / テストの同期点）では予約 off を grace を待たずに
///   フラッシュして終了する（「終了前にキューを掃く」契約の維持。off は要求済みで、
///   以後 on は来ない）。
fn worker_loop<'env, F: Fn(Job<'env>)>(rx: Receiver<Job<'env>>, policy: WorkerPolicy, run: F) {
    let mut pending_off: Option<(Instant, Job<'env>)> = None;
    let mut last_sent: Option<Instant> = None;

    // min-gap を尊重して実行し、送信時刻を記録する。
    let exec = |job: Job<'env>, last_sent: &mut Option<Instant>| {
        if let Some(last) = *last_sent {
            let since = last.elapsed();
            if since < policy.min_gap {
                std::thread::sleep(policy.min_gap - since);
            }
        }
        run(job);
        *last_sent = Some(Instant::now());
    };

    loop {
        let received = match pending_off {
            Some((deadline, job)) => {
                match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(next) => Some(next),
                    Err(RecvTimeoutError::Timeout) => {
                        // 予約満了: on に取り消されなかった off を実行する。
                        pending_off = None;
                        exec(job, &mut last_sent);
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => None,
                }
            }
            None => rx.recv().ok(),
        };

        let Some(job) = received else {
            // 切断: 予約 off をフラッシュして終了。
            if let Some((_, job)) = pending_off.take() {
                tracing::debug!(rule = %job.rule.name, device = job.then.device(),
                    "flushing pending off on shutdown");
                exec(job, &mut last_sent);
            }
            return;
        };

        if grace_applies(&job, policy) {
            tracing::debug!(rule = %job.rule.name, device = job.then.device(),
                grace_ms = policy.off_grace.as_millis() as u64, "holding off in grace");
            pending_off = Some((Instant::now() + policy.off_grace, job));
            continue;
        }
        match job.then {
            Then::On { .. } | Then::Off { .. } => {
                if let Some((_, held)) = pending_off.take() {
                    tracing::debug!(rule = %held.rule.name, device = held.then.device(),
                        superseded_by = job.then.action_name(), "discarding pending off");
                }
                exec(job, &mut last_sent);
            }
            Then::Invoke { .. } => exec(job, &mut last_sent),
        }
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
            active: None,
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
            active: None,
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

    /// イベントトリガ（人感など）のテスト用ルール。アクションは指定の Then。
    fn event_rule(name: &str, then: Then) -> Rule {
        Rule {
            name: name.to_string(),
            when: Trigger::Event {
                device: "sensor".to_string(),
                epc: "0x80".to_string(),
                equals: "0x30".to_string(),
            },
            active: None,
            then: Thens::One(then),
        }
    }

    fn on(device: &str) -> Then {
        Then::On {
            device: device.to_string(),
        }
    }

    fn off(device: &str) -> Then {
        Then::Off {
            device: device.to_string(),
        }
    }

    /// 実行されたアクション名を記録するログ。
    fn action_log() -> Mutex<Vec<&'static str>> {
        Mutex::new(Vec::new())
    }

    #[test]
    fn event_off_in_grace_is_discarded_by_on() {
        // イベント由来の off は grace 中は送信されず、on が来たら破棄される
        // （off→on 連射がデバイスに届かない。issue #5 の本体）。
        let off_rule = event_rule("leave", off("dev_a"));
        let on_rule = event_rule("return", on("dev_a"));
        let policy = WorkerPolicy {
            off_grace: Duration::from_millis(200),
            min_gap: Duration::ZERO,
        };
        let log = action_log();
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], policy, |j: Job| {
                log.lock().unwrap().push(j.then.action_name());
            });
            assert_eq!(d.dispatch(&off_rule), 1);
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(d.dispatch(&on_rule), 1);
            drop(d); // 破棄漏れがあれば切断時フラッシュで off が記録される
        });
        assert_eq!(*log.lock().unwrap(), ["on"]);
    }

    #[test]
    fn event_off_fires_after_grace_expires() {
        // on に取り消されなければ、grace 満了で off が実行される（消灯はちゃんと起きる）。
        let off_rule = event_rule("leave", off("dev_a"));
        let policy = WorkerPolicy {
            off_grace: Duration::from_millis(100),
            min_gap: Duration::ZERO,
        };
        let log = action_log();
        let fired_at = Mutex::new(None::<Duration>);
        let started = Instant::now();
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], policy, |j: Job| {
                log.lock().unwrap().push(j.then.action_name());
                *fired_at.lock().unwrap() = Some(started.elapsed());
            });
            assert_eq!(d.dispatch(&off_rule), 1);
            // grace 満了は Sender 生存中に起きる（切断フラッシュとの区別）。
            std::thread::sleep(Duration::from_millis(250));
            assert_eq!(*log.lock().unwrap(), ["off"]);
            drop(d);
        });
        let elapsed = fired_at.lock().unwrap().unwrap();
        assert!(
            elapsed >= Duration::from_millis(100),
            "off fired before grace expired: {elapsed:?}"
        );
    }

    #[test]
    fn time_triggered_off_runs_immediately_without_grace() {
        // 時刻トリガの off は grace の対象外（22:00 消灯が遅れない）。
        let off_rule = Rule {
            name: "bedtime".to_string(),
            when: Trigger::Time {
                at: "22:00".to_string(),
            },
            active: None,
            then: Thens::One(off("dev_a")),
        };
        let policy = WorkerPolicy {
            off_grace: Duration::from_secs(10),
            min_gap: Duration::ZERO,
        };
        let log = action_log();
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], policy, |j: Job| {
                log.lock().unwrap().push(j.then.action_name());
            });
            assert_eq!(d.dispatch(&off_rule), 1);
            // Sender 生存のまま短時間で実行されること（切断フラッシュではない）。
            let deadline = Instant::now() + Duration::from_millis(500);
            while log.lock().unwrap().is_empty() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(*log.lock().unwrap(), ["off"]);
            drop(d);
        });
    }

    #[test]
    fn min_gap_spaces_consecutive_commands() {
        let first = event_rule("first", on("dev_a"));
        let second = event_rule("second", on("dev_a"));
        let policy = WorkerPolicy {
            off_grace: Duration::ZERO,
            min_gap: Duration::from_millis(100),
        };
        let sent = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], policy, |_j: Job| {
                sent.lock().unwrap().push(Instant::now());
            });
            assert_eq!(d.dispatch(&first), 1);
            assert_eq!(d.dispatch(&second), 1);
            drop(d);
        });
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        let gap = sent[1] - sent[0];
        assert!(gap >= Duration::from_millis(100), "gap too small: {gap:?}");
    }

    #[test]
    fn shutdown_flushes_pending_off_without_waiting_grace() {
        // 切断時は grace を待たずに予約 off を実行して終了する（キューを掃く契約）。
        let off_rule = event_rule("leave", off("dev_a"));
        let policy = WorkerPolicy {
            off_grace: Duration::from_secs(30),
            min_gap: Duration::ZERO,
        };
        let log = action_log();
        let started = Instant::now();
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], policy, |j: Job| {
                log.lock().unwrap().push(j.then.action_name());
            });
            assert_eq!(d.dispatch(&off_rule), 1);
            drop(d);
        });
        assert_eq!(*log.lock().unwrap(), ["off"]);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "shutdown waited for grace"
        );
    }

    #[test]
    fn repeated_event_offs_conflate_into_one() {
        // off 連射は 1 件の予約にまとまり、満了時に 1 回だけ実行される。
        let off_rule = event_rule("leave", off("dev_a"));
        let policy = WorkerPolicy {
            off_grace: Duration::from_millis(100),
            min_gap: Duration::ZERO,
        };
        let log = action_log();
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], policy, |j: Job| {
                log.lock().unwrap().push(j.then.action_name());
            });
            assert_eq!(d.dispatch(&off_rule), 1);
            std::thread::sleep(Duration::from_millis(30));
            assert_eq!(d.dispatch(&off_rule), 1);
            std::thread::sleep(Duration::from_millis(250));
            drop(d);
        });
        assert_eq!(*log.lock().unwrap(), ["off"]);
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
            let d = Dispatcher::new(s, ["dev_a", "dev_b"], WorkerPolicy::ZERO, |j: Job| {
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
            active: None,
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
            let d = Dispatcher::new(s, ["dev_a"], WorkerPolicy::ZERO, |j: Job| {
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
            let d = Dispatcher::new(s, ["dev_a", "dev_b"], WorkerPolicy::ZERO, |j: Job| {
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
            let d = Dispatcher::new(s, ["dev_a"], WorkerPolicy::ZERO, |_j: Job| {});
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
            let d = Dispatcher::new(s, ["dev_a"], WorkerPolicy::ZERO, |j: Job| {
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
            let d = Dispatcher::new(s, ["dev_a"], WorkerPolicy::ZERO, |j: Job| {
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
            let d = Dispatcher::new(s, ["dev_a", "dev_b"], WorkerPolicy::ZERO, |j: Job| {
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
            let d = Dispatcher::new(s, ["dev_a"], WorkerPolicy::ZERO, |_j: Job| {});
            assert_eq!(d.dispatch(&r), 0);
            drop(d);
        });
    }

    #[test]
    fn dispatch_all_sums_actions_across_multiple_multi_action_rules() {
        // 複数の多アクションルールにまたがって積めた件数を合計できること
        // （単一ルールの dispatch だけでは検証できない）。
        let a = multi_rule("a", &["dev_a", "dev_b"]);
        let b = multi_rule("b", &["dev_a", "dev_b", "no_such_device"]);
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a", "dev_b"], WorkerPolicy::ZERO, |_j: Job| {});
            // a: 2 件とも積める。b: dev_a, dev_b は積めるが no_such_device は積めない → 2 件。
            assert_eq!(d.dispatch_all(vec![&a, &b]), 4);
            drop(d);
        });
    }
}
