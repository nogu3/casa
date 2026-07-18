# casad 発火非同期化（デバイス別ワーカー）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** casad 常駐モードのルール発火をデバイス別ワーカーに非同期投入し、`enl listen` の取りこぼし窓を消しつつ異デバイス並列・同一デバイス FIFO を実現する。

**Architecture:** 新モジュール `dispatch.rs` に `Dispatcher`（デバイス名 → `mpsc::Sender<&Rule>` の HashMap + デバイスごとの scoped ワーカースレッド）を置く。ワーカーの実行関数はジェネリクスで注入し、単体テストはプロセスを起動せず Mutex 記録クロージャで順序・並列性を検証する。`engine::run` の常駐経路（event_loop / time_loop）だけを dispatcher 経由に切り替え、`--once` / `--listen-once` は従来どおり同期実行。

**Tech Stack:** Rust (edition 2021), std のみ（`std::sync::mpsc` + `std::thread::scope`）。依存追加なし。

**Spec:** `docs/superpowers/specs/2026-07-17-async-rule-firing-design.md`

## Global Constraints

- 依存 crate を追加しない（std のみ）。
- `cargo clippy -- -D warnings` を常に通す（コミット前に `cargo fmt` も）。
- `--once` / `--listen-once` の同期実行・exit code 意味論を変えない（cron の「終了 = 全アクション完了」）。
- 既存テスト（`crates/casad/tests/{check,events,exec,run}.rs`・各 unit テスト）を無変更で通す。
- フィクスチャは POSIX sh・ダミー値のみ（RFC 5737 `192.0.2.0/24`）。実 IP・実機 ID 禁止。
- ログは既存形式に合わせる: 失敗 warn は `rule action exited nonzero` / `rule action failed` の文言を維持（tests/events.rs が文言に依存）。
- コミットメッセージは既存リポジトリの流儀（日本語・conventional prefix）。

---

### Task 1: `dispatch.rs` — Dispatcher とデバイス別ワーカー

**Files:**
- Create: `crates/casad/src/dispatch.rs`
- Modify: `crates/casad/src/main.rs`（`mod dispatch;` を追加）
- Test: `crates/casad/src/dispatch.rs` 内 `#[cfg(test)]`

**Interfaces:**
- Consumes: `crate::rules::{Rule, RuleFile}`（既存。`Rule { name, when, then }`、`Then::device() -> &str` は実装済み）
- Produces（Task 2 が使う）:
  - `pub struct Dispatcher<'env>`（`#[derive(Clone)]`）
  - `pub fn Dispatcher::new<'scope, F>(scope: &'scope Scope<'scope, 'env>, devices: impl IntoIterator<Item = &'env str>, run: F) -> Dispatcher<'env> where F: Fn(&'env Rule) + Send + Copy + 'scope`
  - `pub fn Dispatcher::dispatch(&self, rule: &'env Rule) -> bool`（キュー投入成否）
  - `pub fn Dispatcher::dispatch_all(&self, rules: Vec<&'env Rule>) -> usize`（投入できた件数）
  - `pub fn distinct_devices(file: &RuleFile) -> BTreeSet<&str>`

- [ ] **Step 1: 失敗するテストを含む dispatch.rs を書く**

`crates/casad/src/dispatch.rs` を以下の内容で作成する（TDD の Red を確認したい場合は、まず `new` / `dispatch` / `dispatch_all` の本体を `todo!()` にしてテストが落ちることを見てから、以下の実装で置き換える。最終形は以下のとおり）:

```rust
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
```

- [ ] **Step 2: main.rs にモジュール宣言を追加**

`crates/casad/src/main.rs` の先頭モジュール群を修正:

```rust
mod casa_runner;
mod cli;
mod dispatch;
mod engine;
mod enl;
mod rules;
```

- [ ] **Step 3: テストを実行して通す**

Run: `cargo test -p casad dispatch`
Expected: 5 テストすべて PASS（`distinct_devices_dedupes_then_targets` / `same_device_actions_run_in_fifo_order` / `dispatch_returns_before_action_completes` / `different_devices_run_in_parallel` / `dispatch_to_unknown_device_returns_false`）。

コンパイルエラーが出る場合はライフタイム注釈（`'scope` / `'env`）の食い違いが典型。`Scope<'scope, 'env>` は `'env: 'scope` を内包するので、`F: 'scope` かつ送るのは `&'env Rule` で成立する。

注意: この時点で `Dispatcher` は未使用のため dead_code 警告が出る。`cargo clippy` はまだ通らなくてよい（Task 2 で使用され解消する）。CI 相当の確認は Task 2 の Step 5 で行う。

- [ ] **Step 4: コミット**

```bash
git add crates/casad/src/dispatch.rs crates/casad/src/main.rs
git commit -m "feat(casad): デバイス別ワーカーの Dispatcher を追加（未配線）

同一デバイス FIFO・異デバイス並列・dispatch 即戻りを単体テストで保証。
実行関数はジェネリクス注入とし、テストはプロセス起動なしで検証する。"
```

---

### Task 2: engine.rs の配線 — 常駐経路を dispatcher 経由に

**Files:**
- Modify: `crates/casad/src/engine.rs`
  - `fire_all`（144-156 行付近）を `run_one` + `fire_all` に分割
  - `fire_due_events`（115-128 行付近）の突合フィルタを `due_event_rules` に抽出
  - `run`（160-188 行付近）/ `time_loop`（191-197 行付近）/ `event_loop`（201-213 行付近）を dispatcher 経由に

**Interfaces:**
- Consumes: Task 1 の `crate::dispatch::{Dispatcher, distinct_devices}`（シグネチャは Task 1 の Produces のとおり）
- Produces: 外部公開 API（`fire` / `tick` / `fire_due_events` / `drain_events_once` / `run` / `validate_schedule` / `parse_hm` / `due_time_rules` / `event_matches`）は**シグネチャ無変更**。`main.rs` の呼び出しは一切変えない。

- [ ] **Step 1: `run_one` を抽出し `fire_all` を書き換える**

`crates/casad/src/engine.rs` の `fire_all` を以下に置き換える:

```rust
/// 1 ルールを実行し、成功（casa が exit 0）なら true。失敗は warn ログに残す。
/// 同期経路（`fire_all`）と非同期ワーカー（dispatcher）の両方がこれを使う。
fn run_one(rule: &Rule, config_path: Option<&Path>) -> bool {
    match fire(rule, config_path) {
        Ok(0) => true,
        Ok(code) => {
            tracing::warn!(rule = %rule.name, code, "rule action exited nonzero");
            false
        }
        Err(e) => {
            tracing::warn!(rule = %rule.name, error = %e, "rule action failed");
            false
        }
    }
}

/// 与えられたルール群を同期・直列にすべて発火する（`--once` / `--listen-once` 用）。
/// 成功した件数を返す。失敗はループを止めない（常駐の頑健性）。
fn fire_all(rules: Vec<&Rule>, config_path: Option<&Path>) -> usize {
    rules
        .into_iter()
        .filter(|rule| run_one(rule, config_path))
        .count()
}
```

- [ ] **Step 2: `due_event_rules` を抽出する**

`fire_due_events` を以下に置き換える（突合フィルタを同期・非同期で共有する）:

```rust
/// 1 バッチの通知に一致するイベントトリガのルールを返す（rules.toml 記載順）。
/// 同じルールが複数通知に一致しても 1 回だけ含む。
fn due_event_rules<'a>(
    file: &'a RuleFile,
    config: &Config,
    events: &[enl::Event],
) -> Vec<&'a Rule> {
    file.rules
        .iter()
        .filter(|r| matches!(r.when, Trigger::Event { .. }))
        .filter(|r| events.iter().any(|e| event_matches(r, config, e)))
        .collect()
}

/// 1 バッチの通知に対し、一致するイベントトリガを同期・直列に発火する
/// （`--listen-once` 用）。発火した件数を返す。
pub fn fire_due_events(
    file: &RuleFile,
    config: &Config,
    events: &[enl::Event],
    config_path: Option<&Path>,
) -> usize {
    fire_all(due_event_rules(file, config, events), config_path)
}
```

- [ ] **Step 3: `run` / `time_loop` / `event_loop` を dispatcher 経由にする**

まず import に追加:

```rust
use crate::dispatch::{distinct_devices, Dispatcher};
```

`run` の常駐部（`std::thread::scope` ブロック）を以下に置き換える:

```rust
    // scope で借用を渡し、Arc/clone なしに 2 ループ + ワーカー群を並行させる。
    // アクション実行はデバイス別ワーカーに非同期投入する（同一デバイス FIFO・
    // 異デバイス並列）。listen / tick ループはアクション完了を待たない。
    std::thread::scope(|s| {
        let dispatcher = Dispatcher::new(s, distinct_devices(file), move |rule: &Rule| {
            run_one(rule, config_path);
        });
        if has_events {
            let d = dispatcher.clone();
            s.spawn(move || event_loop(file, config, enl_bin, &d));
        }
        time_loop(file, &dispatcher);
    });
    Ok(0) // time_loop は戻らないので到達しない。
```

`time_loop` を置き換える:

```rust
/// 時刻スケジューラ。毎分の境界で該当ルールをワーカーに積む。
fn time_loop<'env>(file: &'env RuleFile, dispatcher: &Dispatcher<'env>) -> ! {
    loop {
        let now = Local::now().time();
        let queued = dispatcher.dispatch_all(due_time_rules(file, now));
        if queued > 0 {
            tracing::debug!(queued, "time rules queued");
        }
        sleep_to_next_minute();
    }
}
```

`event_loop` を置き換える:

```rust
/// イベントリスナ。`enl listen` を回し続け、一致ルールをワーカーに積んで即再 listen する。
/// enl 起動失敗・異常終了はバックオフして再試行（常駐の頑健性）。
fn event_loop<'env>(
    file: &'env RuleFile,
    config: &Config,
    enl_bin: &str,
    dispatcher: &Dispatcher<'env>,
) -> ! {
    loop {
        match enl::listen_once(enl_bin) {
            Ok(events) => {
                let queued = dispatcher.dispatch_all(due_event_rules(file, config, &events));
                if queued > 0 {
                    tracing::debug!(queued, "event rules queued");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "enl listen failed; backing off");
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}
```

- [ ] **Step 4: 全テストを実行する**

Run: `cargo test -p casad`
Expected: 既存の unit テスト（engine / rules / dispatch）+ 統合テスト（check / events / exec / run）すべて PASS。`--listen-once` は同期 `fire_due_events` のままなので `tests/events.rs` の文言アサーション（`rule action exited nonzero`）も通る。

- [ ] **Step 5: fmt / clippy を通す**

Run: `cargo fmt && cargo clippy -- -D warnings`
Expected: 警告ゼロ（Task 1 の dead_code もここで解消している）。

- [ ] **Step 6: コミット**

```bash
git add crates/casad/src/engine.rs
git commit -m "feat(casad): 常駐モードの発火をデバイス別ワーカーに非同期化

event_loop / time_loop はルールをキューに積んで即戻り、enl listen の
取りこぼし窓をアクション実行時間から enl 再 spawn の数 ms に短縮する。
時刻×イベントの同一デバイス同時実行もワーカー直列化で解消。
--once / --listen-once は従来どおり同期実行を維持。"
```

---

### Task 3: 常駐モードの統合テスト

**Files:**
- Create: `crates/casad/tests/fixtures/enl_listen_once_then_block.sh`（実行権限付き）
- Create: `crates/casad/tests/fixtures/casa_record.sh`（実行権限付き）
- Create: `crates/casad/tests/resident.rs`

**Interfaces:**
- Consumes: `tests/common/mod.rs` の `write_config` / `fixture` / `DUMMY_CONFIG`（既存）。casad バイナリは `env!("CARGO_BIN_EXE_casad")`。
- Produces: なし（テストのみ）。

- [ ] **Step 1: enl 代役フィクスチャを作る**

`crates/casad/tests/fixtures/enl_listen_once_then_block.sh`:

```sh
#!/bin/sh
# enl 代役（常駐テスト用）。初回起動のみ INF 通知を 1 件出し、
# 2 回目以降（casad の再 spawn）は長時間ブロックする。casad kill 後も
# 残らないよう sleep は 60 秒で自然終了させる。
marker="${CASAD_TEST_DIR:?}/emitted"
if [ -e "$marker" ]; then
  sleep 60
  exit 0
fi
touch "$marker"
echo "{\"events\":[{\"ip\":\"192.0.2.10\",\"tid\":\"00ab\",\"seoj\":\"013001\",\"deoj\":\"05ff01\",\"esv\":\"Inf\",\"properties\":[{\"epc\":\"80\",\"pdc\":1,\"edt_hex\":\"30\"}]}]}"
exit 0
```

`crates/casad/tests/fixtures/casa_record.sh`:

```sh
#!/bin/sh
# casa 代役（常駐テスト用）。呼ばれた引数をファイルに追記する。
# 常駐 casad は exit しないため stdout ではなくファイルで観測する。
echo "casa called: $@" >> "${CASAD_TEST_DIR:?}/casa.log"
exit 0
```

実行権限を付ける:

```bash
chmod +x crates/casad/tests/fixtures/enl_listen_once_then_block.sh crates/casad/tests/fixtures/casa_record.sh
```

- [ ] **Step 2: 統合テストを書く**

`crates/casad/tests/resident.rs`:

```rust
//! 常駐モード（`casad run`、--once / --listen-once なし）の統合テスト。
//! enl 代役が 1 件通知 → デバイス別ワーカー経由で casa 代役が発火することを
//! ファイル観測で検証する。常駐は終了しないため、発火確認後に kill する。

mod common;

use std::time::{Duration, Instant};

use common::*;

const EVENT_RULES: &str = r#"
version = 1
[[rules]]
name = "エアコン電源ONで点灯"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "living_aircon" }
"#;

#[test]
fn resident_event_loop_fires_casa_via_worker() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules_path = dir.path().join("rules.toml");
    std::fs::write(&rules_path, EVENT_RULES).unwrap();

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_casad"))
        .args([
            "run",
            rules_path.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
        ])
        .env_remove("CASA_CONFIG")
        .env("CASA_ENL_BIN", fixture("enl_listen_once_then_block.sh"))
        .env("CASA_BIN", fixture("casa_record.sh"))
        .env("CASAD_TEST_DIR", dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    // ワーカー経由の発火は非同期なのでファイル出現をポーリングで待つ。
    let log = dir.path().join("casa.log");
    let deadline = Instant::now() + Duration::from_secs(10);
    let fired = loop {
        if std::fs::read_to_string(&log)
            .map(|s| s.contains("on living_aircon"))
            .unwrap_or(false)
        {
            break true;
        }
        if Instant::now() > deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    child.kill().unwrap();
    let _ = child.wait();
    assert!(fired, "casa was not fired via worker within 10s");
}
```

- [ ] **Step 3: テストを実行する**

Run: `cargo test -p casad --test resident`
Expected: `resident_event_loop_fires_casa_via_worker` PASS（数百 ms で発火するはず。10s タイムアウトは CI 余裕分）。

- [ ] **Step 4: 全体回帰**

Run: `cargo test -p casad && cargo clippy -- -D warnings`
Expected: すべて PASS・警告ゼロ。

- [ ] **Step 5: コミット**

```bash
git add crates/casad/tests/fixtures/enl_listen_once_then_block.sh crates/casad/tests/fixtures/casa_record.sh crates/casad/tests/resident.rs
git commit -m "test(casad): 常駐モードのワーカー経由発火を統合テストで検証

enl 代役が初回のみ通知を出し以降ブロック、casa 代役はファイル追記で
観測する。常駐プロセスは発火確認後に kill する。"
```

---

### Task 4: ドキュメントとバージョン

**Files:**
- Modify: `CLAUDE.md`（casad 責務セクションのルールエンジン記述）
- Modify: `Cargo.toml`（workspace version `0.6.0` → `0.7.0`）

**Interfaces:**
- Consumes: なし
- Produces: なし（ドキュメントのみ）

- [ ] **Step 1: CLAUDE.md の casad 責務を更新**

`CLAUDE.md` の「`casad` 側が担う責務」セクション、以下の行:

```
- 自動化ルール DSL（`rules.toml`）の評価エンジン — **実装済み**:
  - 時刻トリガ（内部スケジューラ）/ イベントトリガ（`enl listen` をループで回して INF 通知に反応）
```

を以下に置き換える:

```
- 自動化ルール DSL（`rules.toml`）の評価エンジン — **実装済み**:
  - 時刻トリガ（内部スケジューラ）/ イベントトリガ（`enl listen` をループで回して INF 通知に反応）
  - 発火はデバイス別ワーカーへ非同期投入（同一デバイス FIFO・異デバイス並列）。
    アクション実行中も `enl listen` は止まらない。`--once` / `--listen-once` は同期実行。
```

- [ ] **Step 2: workspace バージョンを bump**

`Cargo.toml`（リポジトリルート）の `version = "0.6.0"` を `version = "0.7.0"` に変更する。

- [ ] **Step 3: ビルド確認**

Run: `cargo build && cargo test && cargo clippy -- -D warnings`
Expected: すべて成功（`Cargo.lock` の version 追従もここで入る）。

- [ ] **Step 4: コミット**

```bash
git add CLAUDE.md Cargo.toml Cargo.lock
git commit -m "docs: casad 発火のデバイス別ワーカー非同期化を記載し 0.7.0 に bump"
```

---

## 実装後の運用メモ（プラン外・ユーザー判断）

- jarvis への配布は既存手順（cross build → scp、`jarvis-deploy` メモリ参照）。配布後 `systemctl --user restart casad` と、実 INF での発火ログ確認（`journalctl | grep casad`、ユニットフィルタ不可の罠に注意）。
