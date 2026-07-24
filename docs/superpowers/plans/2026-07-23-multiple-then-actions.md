# casad ルールの複数アクション（`then` 配列）対応 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 1 つのルールの `when` に対して複数の `then` を書けるようにし、各アクションを対象デバイス別ワーカーへファンアウトする。

**Architecture:** `Rule.then` の型を単一の `Then` から `Thens`（単一テーブルと配列の両方を `serde(untagged)` で受ける enum）に変える。ディスパッチの単位を「ルール」から「(ルール, アクション)」= `Job` に変え、ワーカーのキーは従来どおりデバイス名のままにする。これにより同一デバイス宛は記載順の FIFO、別デバイス宛は並列という既存の不変条件が維持される。

**Tech Stack:** Rust 2021 / serde + toml / tracing / std::thread::scope + mpsc（既存構成のまま。新規依存なし）

設計: `docs/superpowers/specs/2026-07-23-multiple-then-actions-design.md`

## Global Constraints

- ワークスペースは `crates/casa-core` / `crates/casa` / `crates/casad` の 3 crate。本対応で触るのは `crates/casad` のみ（`casa-core` / `casa` は無変更）。
- 新規 crate 依存を足さない。
- `rules.toml` の `version` は **1 のまま**。`SUPPORTED_VERSION` は変更しない。
- 既存の単一テーブル記法 `then = { action = "...", device = "..." }` は動き続けること。jarvis で稼働中の rules.toml がこの記法。
- 守るべき不変条件: **同一デバイス宛アクションの FIFO 順序**（`crates/casad/src/dispatch.rs` のワーカーがデバイス名をキーに張られていることで実現）。
- `Then` の 3 バリアント（`On` / `Off` / `Invoke`）と `Then::device()` / `Then::casa_args()` のシグネチャは変更しない。`crates/casad/src/cli.rs` の `ExecAction::into_then()`（`casad exec` 用）はこれに依存しているので壊さない。
- 各タスクの完了時に `cargo test` と `cargo clippy -- -D warnings` が通ること。
- コード中のコメント・ドキュメント文字列は既存に合わせて日本語。README のみ英語。

---

### Task 1: `Thens` 型 — パース・シリアライズ・検証

`then` が単一テーブルと配列の両方を受け付けるようにする。この時点では `dispatch` / `fire` は先頭アクションのみを使う暫定実装のままにし、ファンアウトは Task 2 で入れる（中間状態でも常に単一 then 相当の正しい動作をする）。

**Files:**
- Modify: `crates/casad/src/rules.rs`（型定義 27-32 行付近、`parse` 111-130 行、`validate` 170-185 行、テスト 228 行以降）
- Modify: `crates/casad/src/dispatch.rs:19-21, 57-73`（コンパイル追随）
- Modify: `crates/casad/src/engine.rs:67-73`（コンパイル追随）

**Interfaces:**
- Consumes: 既存の `Then`（`On` / `Off` / `Invoke`）、`Then::device()`、`Then::casa_args()`
- Produces:
  - `pub enum Thens { One(Then), Many(Vec<Then>) }`（`crates/casad/src/rules.rs`）
  - `pub fn Thens::actions(&self) -> &[Then]` — 記載順のアクション列
  - `Rule.then` の型が `Then` から `Thens` に変わる

- [ ] **Step 1: 失敗するテストを書く**

`crates/casad/src/rules.rs` の `mod tests` の末尾に追加する。

```rust
    #[test]
    fn parses_then_array_in_declaration_order() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "人感ONでまとめて点灯"
when = { at = "18:00" }
then = [
  { action = "on", device = "hallway_light" },
  { action = "invoke", device = "hallway_light", command = "color-temp", args = ["--kelvin", "2700"] },
  { action = "off", device = "entry_motion" },
]
"#,
        )
        .unwrap();
        let actions = file.rules[0].then.actions();
        assert_eq!(actions.len(), 3);
        assert!(matches!(actions[0], Then::On { .. }));
        assert!(matches!(actions[1], Then::Invoke { .. }));
        assert!(matches!(actions[2], Then::Off { .. }));
        assert_eq!(actions[0].device(), "hallway_light");
        assert_eq!(actions[2].device(), "entry_motion");
    }

    #[test]
    fn single_then_table_yields_one_action() {
        let file = parse(VALID).unwrap();
        let actions = file.rules[0].then.actions();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Then::On { .. }));
    }

    #[test]
    fn single_and_array_forms_can_coexist_in_one_file() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "単一形"
when = { at = "07:00" }
then = { action = "on", device = "hallway_light" }
[[rules]]
name = "配列形"
when = { at = "22:00" }
then = [
  { action = "off", device = "hallway_light" },
  { action = "off", device = "entry_motion" },
]
"#,
        )
        .unwrap();
        assert_eq!(file.rules[0].then.actions().len(), 1);
        assert_eq!(file.rules[1].then.actions().len(), 2);
    }

    #[test]
    fn empty_then_array_is_config_parse_error_naming_the_rule() {
        let err = parse(
            r#"
version = 1
[[rules]]
name = "空のthen"
when = { at = "07:00" }
then = []
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(
            err.detail.contains("空のthen"),
            "detail should name the rule: {}",
            err.detail
        );
    }

    #[test]
    fn undefined_device_anywhere_in_then_array_fails_validation() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "2番目が未定義"
when = { at = "07:00" }
then = [
  { action = "on", device = "hallway_light" },
  { action = "on", device = "no_such_device" },
]
"#,
        )
        .unwrap();
        let err = file.validate(&config_with_devices()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NameNotFound);
        assert!(
            err.detail.contains("2番目が未定義") && err.detail.contains("no_such_device"),
            "detail should name rule and device: {}",
            err.detail
        );
    }

    #[test]
    fn then_round_trips_through_json_preserving_shape() {
        // casad check は RuleFile をそのまま JSON 出力する。単一形はオブジェクト、
        // 配列形は配列のまま出ること（既存出力の互換維持）。
        let file = parse(
            r#"
version = 1
[[rules]]
name = "単一形"
when = { at = "07:00" }
then = { action = "on", device = "hallway_light" }
[[rules]]
name = "配列形"
when = { at = "22:00" }
then = [{ action = "off", device = "hallway_light" }]
"#,
        )
        .unwrap();
        let json = serde_json::to_value(&file).unwrap();
        assert!(json["rules"][0]["then"].is_object());
        assert!(json["rules"][1]["then"].is_array());
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p casad --lib rules`
Expected: コンパイルエラー（`no method named 'actions' found for enum 'Then'`）。既存テストも `matches!(file.rules[0].then, Then::On { .. })` のままなので、この時点ではまだ通っている。

- [ ] **Step 3: `Thens` を実装する**

`crates/casad/src/rules.rs` の `Rule` 定義（27-32 行）を差し替える。

```rust
#[derive(Debug, Deserialize, Serialize)]
pub struct Rule {
    pub name: String,
    pub when: Trigger,
    pub then: Thens,
}

/// 1 ルールのアクション列。TOML では単一テーブルと配列の両方を受ける（untagged）。
/// - `then = { action = "on", device = "a" }`
/// - `then = [{ action = "on", device = "a" }, { action = "on", device = "b" }]`
///
/// 呼び出し側は本 enum を直接 match せず、必ず [`Thens::actions`] を経由する
/// （単一形 / 配列形の非対称を呼び出し側に漏らさないため）。
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Thens {
    One(Then),
    Many(Vec<Then>),
}

impl Thens {
    /// 記載順のアクション列。単一形は 1 要素のスライスとして見せる。
    pub fn actions(&self) -> &[Then] {
        match self {
            Thens::One(t) => std::slice::from_ref(t),
            Thens::Many(v) => v,
        }
    }
}
```

`parse`（111-130 行）のバージョン検証の直後、`Ok(file)` の前に空配列チェックを足す。何も起きないルールは書き間違い以外にありえないため、設定参照の検証（`validate`）ではなく構造の検証としてここで弾く。

```rust
    for rule in &file.rules {
        if rule.then.actions().is_empty() {
            return Err(CasaError::new(
                ErrorKind::ConfigParse,
                format!("rule \"{}\": then is empty", rule.name),
            ));
        }
    }

    Ok(file)
```

`validate`（181 行）の `check_target` 呼び出しを全アクションに回す。

```rust
            for then in rule.then.actions() {
                check_target(config, &rule.name, then.device())?;
            }
```

- [ ] **Step 4: 既存テストを `actions()` 経由に直す**

`crates/casad/src/rules.rs` の既存テスト 3 箇所。`Rule.then` を直接 match している箇所を潰す。

269 行:
```rust
        assert!(matches!(file.rules[0].then.actions()[0], Then::On { .. }));
```

284 行:
```rust
        match &file.rules[0].then.actions()[0] {
```

310 行:
```rust
        match &file.rules[0].then.actions()[0] {
```

- [ ] **Step 5: 呼び出し側をコンパイルが通る形にする（暫定）**

`crates/casad/src/dispatch.rs:19-21` の `distinct_devices` は全アクションを集める形に変える（これは Task 2 でも変わらない最終形）。

```rust
/// rules.toml の全 `then` アクションの対象名の distinct 集合（BTreeSet で順序決定的）。
pub fn distinct_devices(file: &RuleFile) -> BTreeSet<&str> {
    file.rules
        .iter()
        .flat_map(|r| r.then.actions())
        .map(|t| t.device())
        .collect()
}
```

`crates/casad/src/dispatch.rs:57-63` の `dispatch` 冒頭を暫定実装にする。

```rust
    pub fn dispatch(&self, rule: &'env Rule) -> bool {
        // TODO(Task 2): (ルール, アクション) 単位のファンアウトに置き換える。
        // 現状は先頭アクションのみを積む（単一 then のルールでは従来と同じ挙動）。
        let Some(then) = rule.then.actions().first() else {
            tracing::warn!(rule = %rule.name, "rule has no action; dropping");
            return false;
        };
        let device = then.device();
```

`crates/casad/src/engine.rs:67-73` の `fire` を暫定実装にする。

```rust
/// 1 つのルールの `then` を casa の spawn で実行する。
/// `config_path` は casa へ渡す `--config`（None なら casa が既定パスを解決）。
pub fn fire(rule: &Rule, config_path: Option<&Path>) -> Result<i32, CasaError> {
    // TODO(Task 2): (ルール, アクション) 単位に置き換える。現状は先頭アクションのみ。
    let Some(then) = rule.then.actions().first() else {
        return Ok(0);
    };
    let args = then.casa_args(config_path);
    tracing::info!(rule = %rule.name, "firing rule");
    casa_runner::run_casa(&args)
}
```

`crates/casad/src/dispatch.rs` のテストヘルパ `fn rule()`（90-100 行）の `then` フィールドを `Thens` に合わせる。

```rust
            then: Thens::One(Then::On {
                device: device.to_string(),
            }),
```

同ファイル 87 行の `use crate::rules::{Then, Trigger};` を `use crate::rules::{Then, Thens, Trigger};` にする。

- [ ] **Step 6: テストが通ることを確認する**

Run: `cargo test -p casad`
Expected: PASS（新規 6 件を含む全件）

Run: `cargo clippy -- -D warnings`
Expected: 警告なし

- [ ] **Step 7: コミットする**

```bash
git add crates/casad/src/rules.rs crates/casad/src/dispatch.rs crates/casad/src/engine.rs
git commit -m "feat(casad): then に配列記法を追加（Thens 型・検証）"
```

---

### Task 2: アクション単位のファンアウト

ディスパッチと同期発火の単位を `Job`（ルール + アクション）に変える。Task 1 で入れた暫定実装（先頭アクションのみ）を除去する。

**Files:**
- Modify: `crates/casad/src/dispatch.rs`（`Job` 追加、`Dispatcher` のチャネル型・`dispatch` / `dispatch_all`、テスト）
- Modify: `crates/casad/src/engine.rs`（`fire` / `run_one` / `fire_all` / `run` 内の `Dispatcher::new`、テスト）

**Interfaces:**
- Consumes: `Thens::actions()`（Task 1）
- Produces:
  - `pub struct Job<'env> { pub rule: &'env Rule, pub then: &'env Then }`（`crates/casad/src/dispatch.rs`、`#[derive(Clone, Copy)]`）
  - `Dispatcher::new` のクロージャ境界が `F: Fn(&'env Rule)` から `F: Fn(Job<'env>)` に変わる
  - `Dispatcher::dispatch(&self, rule: &'env Rule) -> usize`（戻り値が `bool` から「積めたアクション件数」に変わる）
  - `Dispatcher::dispatch_all(&self, rules: Vec<&'env Rule>) -> usize`（同上、アクション件数の合計）
  - `engine::fire(job: Job<'_>, config_path: Option<&Path>) -> Result<i32, CasaError>`

- [ ] **Step 1: 失敗するテストを書く（dispatch）**

`crates/casad/src/dispatch.rs` の `mod tests` に追加する。既存の記録クロージャ方式に合わせる。まずヘルパを 1 つ足す。

```rust
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
```

既存テストのうち戻り値の型が変わるものを直す。`assert!(d.dispatch(&slow))` の形が 4 箇所（`same_device_actions_run_in_fifo_order` に 2、`dispatch_returns_before_action_completes` に 1、`different_devices_run_in_parallel` に 2、`dispatch_to_unknown_device_returns_false` に 1）。

```rust
        assert_eq!(d.dispatch(&slow), 1);
        assert_eq!(d.dispatch(&fast), 1);
```

`dispatch_to_unknown_device_returns_false` は名前と本体を直す。

```rust
    #[test]
    fn dispatch_to_unknown_device_counts_zero() {
        let r = rule("ghost", "no_such_device");
        std::thread::scope(|s| {
            let d = Dispatcher::new(s, ["dev_a"], |_j: Job| {});
            assert_eq!(d.dispatch(&r), 0);
            drop(d);
        });
    }
```

既存テストのクロージャ引数もすべて `|r: &Rule|` から `|j: Job|` に変わる。本体で `r.name` を使っている箇所は `j.rule.name`、`r.then.device()` は `j.then.device()` に読み替える。

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p casad --lib dispatch`
Expected: コンパイルエラー（`cannot find type 'Job' in this scope`）

- [ ] **Step 3: `Job` とファンアウトを実装する（dispatch）**

`crates/casad/src/dispatch.rs` の `use` を直す。

```rust
use crate::rules::{Rule, RuleFile, Then};
```

`Dispatcher` の直前に `Job` を足す。

```rust
/// ワーカーに積む仕事の単位。ルールは発火ログ用に持ち回る。
#[derive(Clone, Copy)]
pub struct Job<'env> {
    pub rule: &'env Rule,
    pub then: &'env Then,
}
```

`Dispatcher` のチャネル型とクロージャ境界を `Job` に変える。

```rust
#[derive(Clone)]
pub struct Dispatcher<'env> {
    senders: HashMap<String, Sender<Job<'env>>>,
}

impl<'env> Dispatcher<'env> {
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
```

`dispatch` を 2 段に分ける。Task 1 の暫定コード（先頭アクションのみ）はここで消える。

```rust
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
```

- [ ] **Step 4: dispatch のテストが通ることを確認する**

Run: `cargo test -p casad --lib dispatch`
Expected: PASS

- [ ] **Step 5: 失敗するテストを書く（engine）**

`crates/casad/src/engine.rs` の `mod tests` に追加する。casa を spawn せずに検証するため、仕事列の展開と実行ループを分離した内部関数を対象にする。

```rust
    const MULTI: &str = r#"
version = 1
[[rules]]
name = "まとめて点灯"
when = { at = "07:00" }
then = [
  { action = "on", device = "living_aircon" },
  { action = "invoke", device = "living_aircon", command = "color-temp", args = ["--kelvin", "2700"] },
  { action = "on", device = "entry_lock" },
]
"#;

    #[test]
    fn jobs_expands_actions_in_declaration_order() {
        let file = rules(MULTI);
        let due: Vec<&Rule> = file.rules.iter().collect();
        let jobs = jobs(&due);
        assert_eq!(jobs.len(), 3);
        assert_eq!(jobs[0].then.device(), "living_aircon");
        assert!(matches!(jobs[0].then, Then::On { .. }));
        assert!(matches!(jobs[1].then, Then::Invoke { .. }));
        assert_eq!(jobs[2].then.device(), "entry_lock");
        // ルール名は全 Job が持ち回る（発火ログ用）。
        assert!(jobs.iter().all(|j| j.rule.name == "まとめて点灯"));
    }

    #[test]
    fn fire_jobs_continues_after_a_failing_action() {
        let file = rules(MULTI);
        let due: Vec<&Rule> = file.rules.iter().collect();
        let attempted = std::sync::Mutex::new(Vec::new());
        let ok = fire_jobs(jobs(&due), |job| {
            attempted.lock().unwrap().push(job.then.device().to_string());
            // 2 番目（invoke）だけ失敗させる。
            !matches!(job.then, Then::Invoke { .. })
        });
        // 失敗しても後続は実行される。
        assert_eq!(
            *attempted.lock().unwrap(),
            ["living_aircon", "living_aircon", "entry_lock"]
        );
        // 戻り値は成功したアクション数。
        assert_eq!(ok, 2);
    }
```

`Then` は `engine.rs` の先頭では import していないので、`mod tests` の `use super::*;` の直後に足す。

```rust
    use crate::rules::Then;
```

- [ ] **Step 6: テストが失敗することを確認する**

Run: `cargo test -p casad --lib engine`
Expected: コンパイルエラー（`cannot find function 'jobs' in this scope`）

- [ ] **Step 7: engine を `Job` ベースに実装する**

`crates/casad/src/engine.rs` の `use` に `Job` を足す。

```rust
use crate::dispatch::{distinct_devices, Dispatcher, Job};
```

`fire`（67-73 行）を `Job` を取る形にする。Task 1 の暫定コードはここで消える。

```rust
/// 1 アクションを casa の spawn で実行する。
/// `config_path` は casa へ渡す `--config`（None なら casa が既定パスを解決）。
pub fn fire(job: Job<'_>, config_path: Option<&Path>) -> Result<i32, CasaError> {
    let args = job.then.casa_args(config_path);
    tracing::info!(rule = %job.rule.name, "firing rule");
    casa_runner::run_casa(&args)
}
```

`run_one`（218-232 行）を `Job` を取る形にする。

```rust
/// 1 アクションを実行し、成功（casa が exit 0）なら true。失敗は warn ログに残す。
/// 同期経路（`fire_all`）と非同期ワーカー（dispatcher）の両方がこれを使う。
fn run_one(job: Job<'_>, config_path: Option<&Path>) -> bool {
    match fire(job, config_path) {
        Ok(0) => true,
        Ok(code) => {
            tracing::warn!(rule = %job.rule.name, code, "rule action exited nonzero");
            false
        }
        Err(e) => {
            tracing::warn!(rule = %job.rule.name, error = %e, "rule action failed");
            false
        }
    }
}
```

`fire_all`（234-241 行）を、展開と実行ループに分ける。実行関数を注入できる形にすることで、casa を spawn せずに失敗継続をテストできる。

```rust
/// ルール群を (ルール, アクション) の仕事列へ**記載順**に展開する。
fn jobs<'a>(rules: &[&'a Rule]) -> Vec<Job<'a>> {
    rules
        .iter()
        .copied()
        .flat_map(|rule| {
            rule.then
                .actions()
                .iter()
                .map(move |then| Job { rule, then })
        })
        .collect()
}

/// 仕事列を同期・直列にすべて実行する。成功した件数を返す。
/// 失敗はループを止めない（常駐の頑健性）。`run` は本番では [`run_one`]、
/// テストでは記録クロージャを渡す。
fn fire_jobs<'a, F: Fn(Job<'a>) -> bool>(jobs: Vec<Job<'a>>, run: F) -> usize {
    jobs.into_iter().filter(|job| run(*job)).count()
}

/// 与えられたルール群のアクションを同期・直列にすべて発火する
/// （`--once` / `--listen-once` / `--listen-once-mat` 用）。
/// 成功した**アクション**数を返す（ルール数ではない）。
fn fire_all(rules: Vec<&Rule>, config_path: Option<&Path>) -> usize {
    fire_jobs(jobs(&rules), |job| run_one(job, config_path))
}
```

`run` 内の `Dispatcher::new`（274-276 行）のクロージャ引数を `Job` にする。

```rust
        let dispatcher = Dispatcher::new(s, distinct_devices(file), move |job: Job| {
            run_one(job, config_path);
        });
```

- [ ] **Step 8: 全テストが通ることを確認する**

Run: `cargo test -p casad`
Expected: PASS

Run: `cargo clippy -- -D warnings`
Expected: 警告なし

`crates/casad/src/dispatch.rs` と `crates/casad/src/engine.rs` に `TODO(Task 2)` が残っていないことを確認する。

Run: `rg -n "TODO\(Task 2\)" crates/`
Expected: ヒットなし

- [ ] **Step 9: コミットする**

```bash
git add crates/casad/src/dispatch.rs crates/casad/src/engine.rs
git commit -m "feat(casad): ルールのアクションをデバイス別ワーカーへファンアウト"
```

---

### Task 3: 発火ログにアクション識別子を足す

多 then ルールの発火が journal 上で 1 行に潰れると、どのアクションが走ったか・落ちたかを追えない。発火ログと失敗ログに対象デバイスとアクション種別を足す。

**Files:**
- Modify: `crates/casad/src/rules.rs`（`impl Then` に `action_name` を追加、テスト）
- Modify: `crates/casad/src/engine.rs`（`fire` / `run_one` のログ 3 箇所）

**Interfaces:**
- Consumes: `Job`（Task 2）
- Produces: `pub fn Then::action_name(&self) -> &'static str` — TOML の `action` の値と一致する種別名（`"on"` / `"off"` / `"invoke"`）

- [ ] **Step 1: 失敗するテストを書く**

`crates/casad/src/rules.rs` の `mod tests` に追加する。

```rust
    #[test]
    fn action_name_matches_the_toml_action_value() {
        assert_eq!(
            Then::On {
                device: "a".into()
            }
            .action_name(),
            "on"
        );
        assert_eq!(
            Then::Off {
                device: "a".into()
            }
            .action_name(),
            "off"
        );
        assert_eq!(
            Then::Invoke {
                device: "a".into(),
                command: "blink".into(),
                args: vec![],
            }
            .action_name(),
            "invoke"
        );
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p casad --lib action_name`
Expected: コンパイルエラー（`no method named 'action_name' found`）

- [ ] **Step 3: `action_name` を実装する**

`crates/casad/src/rules.rs` の `impl Then` に足す（`device()` の直後）。

```rust
    /// ログ用のアクション種別名。TOML の `action` の値と一致させる。
    pub fn action_name(&self) -> &'static str {
        match self {
            Then::On { .. } => "on",
            Then::Off { .. } => "off",
            Then::Invoke { .. } => "invoke",
        }
    }
```

- [ ] **Step 4: ログにフィールドを足す**

`crates/casad/src/engine.rs` の `fire` の `tracing::info!`。

```rust
    tracing::info!(
        rule = %job.rule.name,
        device = job.then.device(),
        action = job.then.action_name(),
        "firing rule"
    );
```

`run_one` の warn 2 箇所。どの then が落ちたか判別できないと多 then ルールのデバッグができないため、失敗ログにも同じフィールドを入れる。

```rust
        Ok(code) => {
            tracing::warn!(
                rule = %job.rule.name,
                device = job.then.device(),
                action = job.then.action_name(),
                code,
                "rule action exited nonzero"
            );
            false
        }
        Err(e) => {
            tracing::warn!(
                rule = %job.rule.name,
                device = job.then.device(),
                action = job.then.action_name(),
                error = %e,
                "rule action failed"
            );
            false
        }
```

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test -p casad`
Expected: PASS

Run: `cargo clippy -- -D warnings`
Expected: 警告なし

- [ ] **Step 6: コミットする**

```bash
git add crates/casad/src/rules.rs crates/casad/src/engine.rs
git commit -m "feat(casad): 発火・失敗ログに device / action を足す"
```

---

### Task 4: ドキュメントとバージョン

**Files:**
- Modify: `README.md:366-388`（rules.toml のサンプルブロック）
- Modify: `examples/rules.toml`（末尾に例を追加）
- Modify: `CLAUDE.md:217-218`（casad の `then` の説明）
- Modify: `Cargo.toml:6`（workspace version）

**Interfaces:**
- Consumes: Task 1-3 の実装（配列記法、ファンアウト、ログ）
- Produces: なし（ドキュメントのみ）

- [ ] **Step 1: README にサンプルを足す**

`README.md` の rules.toml サンプルブロック末尾（388 行の閉じ ``` の直前）に追加する。

```toml

# Multiple actions: one trigger, several actions. Same-device actions run in
# declaration order; different-device actions run in parallel.
[[rules]]
name = "desk lights on when study becomes occupied"
when = { device = "study_motion", attribute = "occupancy", equals = 1 }
then = [
  { action = "on", device = "desk_tape_light" },
  { action = "on", device = "desk_light" },
  { action = "invoke", device = "desk_light", command = "color-temp", args = ["--kelvin", "2700"] },
]
```

同ブロックの閉じ ``` の直後に段落を追加する。

```markdown
`then` accepts either a single table or an array of them. With an array, each
action is dispatched to its target device's worker: actions aimed at the same
device run in declaration order, actions aimed at different devices run in
parallel. A failing action does not stop the remaining ones. Use a group
(`[groups.x] members = [...]`) instead when every target takes the same action.
```

- [ ] **Step 2: examples/rules.toml に例を足す**

`examples/rules.toml` の末尾（23 行の後）に追加する。

```toml

# Multiple actions for one trigger. Same-device actions keep declaration order;
# different-device actions run in parallel. `then = []` is a config error.
[[rules]]
name = "example multiple actions"
when = { device = "living_light", attribute = "onoff", equals = true }
then = [
  { action = "on", device = "bedroom_light" },
  { action = "invoke", device = "bedroom_light", command = "color-temp", args = ["--kelvin", "2700"] },
]
```

- [ ] **Step 3: サンプルが実際に検証を通ることを確認する**

Run: `cargo run -p casad -- check examples/rules.toml --config examples/devices.toml`
Expected: exit 0。stdout の JSON で `count` が 4 になり、追加したルールの `then` が JSON 配列として出ること。

- [ ] **Step 4: CLAUDE.md を更新する**

`CLAUDE.md:217-218` を差し替える。

```markdown
  - On firing, casa is called as a child process (`casad run` / `check`). `then` supports `on` / `off` / `invoke`
    (`invoke` takes `device` / `command` / arbitrary `args` and delegates to `casa invoke`). `then` accepts either a
    single table or an array of actions; array members are dispatched per target device (same device = declaration
    order, different devices = parallel), and a failing action does not stop the rest.
```

- [ ] **Step 5: バージョンを上げる**

`Cargo.toml:6` を差し替える。

```toml
version = "1.2.0"
```

Run: `cargo build`
Expected: `Cargo.lock` の casa / casa-core / casad が 1.2.0 に更新される

- [ ] **Step 6: 全体を確認する**

Run: `cargo test`
Expected: PASS

Run: `cargo clippy -- -D warnings`
Expected: 警告なし

- [ ] **Step 7: コミットする**

```bash
git add README.md examples/rules.toml CLAUDE.md Cargo.toml Cargo.lock
git commit -m "docs: then 配列記法を文書化し 1.2.0 に bump"
```

---

## 完了後の運用（本計画の対象外）

jarvis 上の `~/.config/casa/rules.toml` は現在、書斎の人感トリガに対し ON/OFF × (`desk_tape_light`, `desk_light`) の 4 ルールを持つ。これを 2 ルールへ集約するかは運用判断であり、本計画の完了条件に含めない。集約する場合は casad 1.2.0 を jarvis へ配ってから行う（despliegue skill）。設定ファイル自体の IaC 化は jarvis-iac の issue #3 で別途扱う。
