# ルールの有効時間帯（`active` ウィンドウ）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** casad のルールに「有効な時間帯」を持たせ、書斎の人感センサー連動の扇風機を 06:00〜21:00 の間だけ動かせるようにする。

**Architecture:** `Rule` にオプショナルな `active = { from, to }`（インラインテーブル）を追加する。窓の判定は発火判定を担う 3 つのマッチ関数（`due_time_rules` / `due_event_rules` / `due_matter_event_rules`）に集約し、それ以外の発火経路には条件を散らさない。時刻は純粋関数への引数として注入し、常駐ループが `Local::now()` を渡す。`Trigger` の untagged enum には一切触らない。

**Tech Stack:** Rust 2021 / serde + toml / chrono（`NaiveTime`）/ clap derive / tracing

## Global Constraints

- 対象クレートは **casad のみ**。`casa-core` と `casa(bin)` は無改修（配布物も casad だけ）。
- rules.toml の `version` は **1 のまま**（オプショナルフィールドの追加なので後方互換）。
- 区間は **`from` を含み `to` を含まない半開区間 [from, to)**。`from > to` は日跨ぎ。`from == to` はエラー。
- `active` を持たないルールの `casad check` JSON 出力は**現状と完全に一致**すること（`skip_serializing_if`）。
- workspace version: `1.3.0` → **`1.4.0`**（`Cargo.toml` の `[workspace.package] version`）。
- 実機の扇風機: devices.toml 名 `study_fan` / `protocol = "switchbot"` / `device_id = "01-202607251040-40751692"`。
- 実機の有効時間帯: `{ from = "06:00", to = "21:00" }`。
- 各タスクの最後に `cargo clippy --workspace -- -D warnings` が通ること（CI と同条件）。
- コミットは**そのタスクで編集したファイルのみ** `git add` する。

---

### Task 1: `ActiveWindow` 型と rules.toml のパース

**Files:**
- Modify: `crates/casad/src/rules.rs:27-32`（`Rule` 構造体）、同ファイルの `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: なし（最初のタスク）
- Produces:
  - `pub struct ActiveWindow { pub from: String, pub to: String }`（`crates/casad/src/rules.rs`）
  - `Rule.active: Option<ActiveWindow>`

- [ ] **Step 1: 失敗するテストを書く**

`crates/casad/src/rules.rs` の `mod tests` の末尾に追記する:

```rust
    #[test]
    fn parses_active_window() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "昼だけ扇風機"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
active = { from = "06:00", to = "21:00" }
then = { action = "on", device = "study_fan" }
"#,
        )
        .unwrap();
        let w = file.rules[0].active.as_ref().expect("active が None");
        assert_eq!(w.from, "06:00");
        assert_eq!(w.to, "21:00");
    }

    #[test]
    fn active_window_is_none_when_absent() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "常時"
when = { at = "22:00" }
then = { action = "off", device = "living_aircon" }
"#,
        )
        .unwrap();
        assert!(file.rules[0].active.is_none());
    }

    #[test]
    fn active_window_requires_both_ends() {
        // 片側だけの窓は解釈が割れるので受け付けない。
        let err = parse(
            r#"
version = 1
[[rules]]
name = "片側だけ"
when = { at = "22:00" }
active = { from = "06:00" }
then = { action = "off", device = "living_aircon" }
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
    }

    #[test]
    fn serialized_rule_omits_absent_active_window() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "常時"
when = { at = "22:00" }
then = { action = "off", device = "living_aircon" }
[[rules]]
name = "昼だけ"
when = { at = "12:00" }
active = { from = "06:00", to = "21:00" }
then = { action = "off", device = "living_aircon" }
"#,
        )
        .unwrap();
        let v = serde_json::to_value(&file.rules).unwrap();
        // active を持たないルールの JSON は従来どおり（casad check の後方互換）。
        assert!(v[0].get("active").is_none());
        assert_eq!(v[1]["active"]["from"], "06:00");
        assert_eq!(v[1]["active"]["to"], "21:00");
    }
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p casad --bin casad rules::tests::parses_active_window`
Expected: コンパイルエラー `no field 'active' on type '&Rule'`

- [ ] **Step 3: 最小の実装を書く**

`crates/casad/src/rules.rs` の `Rule` を書き換え、直後に `ActiveWindow` を足す:

```rust
#[derive(Debug, Deserialize, Serialize)]
pub struct Rule {
    pub name: String,
    pub when: Trigger,
    /// ルールが有効な時間帯。未指定なら常時有効。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<ActiveWindow>,
    pub then: Thens,
}

/// ルールの有効な時間帯。`from` を含み `to` を含まない半開区間 [from, to)。
/// `from > to` は日跨ぎ（例 21:00-06:00 = 21:00〜23:59 と 00:00〜05:59）。
///
/// HH:MM の書式検証は engine 側（`validate_schedule`）が担う。`Trigger::Time { at }` を
/// String のまま持ち engine で検証しているのと責務の置き場所を揃える。
#[derive(Debug, Deserialize, Serialize)]
pub struct ActiveWindow {
    pub from: String,
    pub to: String,
}
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test -p casad --bin casad rules::`
Expected: 新規 4 件を含め全て PASS

- [ ] **Step 5: clippy を通す**

Run: `cargo clippy --workspace -- -D warnings`
Expected: warning なしで終了（exit 0）

- [ ] **Step 6: コミット**

```bash
git add crates/casad/src/rules.rs
git commit -m "feat(casad): ルールにオプショナルな active ウィンドウを追加

Rule に active = { from, to } を足す。未指定は常時有効で、
skip_serializing_if により既存ルールの casad check JSON 出力は不変。"
```

---

### Task 2: 窓の判定と起動前検証

**Files:**
- Modify: `crates/casad/src/engine.rs:18`（use 文）、`crates/casad/src/engine.rs:39-49`（`validate_schedule`）、同ファイルの `mod tests`

**Interfaces:**
- Consumes: `crate::rules::ActiveWindow`、`Rule.active`（Task 1）
- Produces:
  - `pub fn parse_active_window(w: &ActiveWindow) -> Result<(NaiveTime, NaiveTime), CasaError>`
  - `fn rule_is_active(rule: &Rule, now: NaiveTime) -> bool`（private。Task 3 が使う）
  - `validate_schedule` が `active` も検証するようになる

- [ ] **Step 1: 失敗するテストを書く**

`crates/casad/src/engine.rs` の `mod tests` の末尾に追記する:

```rust
    fn rule_with_window(from: &str, to: &str) -> RuleFile {
        rules(&format!(
            r#"
version = 1
[[rules]]
name = "窓つき"
when = {{ at = "12:00" }}
active = {{ from = "{from}", to = "{to}" }}
then = {{ action = "on", device = "living_aircon" }}
"#
        ))
    }

    #[test]
    fn active_window_includes_from_and_excludes_to() {
        let file = rule_with_window("06:00", "21:00");
        let r = &file.rules[0];
        assert!(rule_is_active(r, at(6, 0)), "from 境界は含む");
        assert!(rule_is_active(r, at(12, 34)));
        assert!(rule_is_active(r, at(20, 59)));
        assert!(!rule_is_active(r, at(21, 0)), "to 境界は含まない");
        assert!(!rule_is_active(r, at(5, 59)));
        assert!(!rule_is_active(r, at(23, 0)));
    }

    #[test]
    fn active_window_wraps_over_midnight() {
        let file = rule_with_window("21:00", "06:00");
        let r = &file.rules[0];
        assert!(rule_is_active(r, at(21, 0)));
        assert!(rule_is_active(r, at(23, 59)));
        assert!(rule_is_active(r, at(0, 0)));
        assert!(rule_is_active(r, at(5, 59)));
        assert!(!rule_is_active(r, at(6, 0)));
        assert!(!rule_is_active(r, at(12, 0)));
    }

    #[test]
    fn rule_without_active_window_is_always_active() {
        let file = rules(SCHEDULE);
        for r in &file.rules {
            assert!(rule_is_active(r, at(0, 0)));
            assert!(rule_is_active(r, at(13, 37)));
            assert!(rule_is_active(r, at(23, 59)));
        }
    }

    #[test]
    fn validate_schedule_rejects_malformed_active_window() {
        let file = rule_with_window("6am", "21:00");
        let err = validate_schedule(&file).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("窓つき"), "detail: {}", err.detail);
    }

    #[test]
    fn validate_schedule_rejects_zero_width_active_window() {
        // from == to は「空区間」とも「全日」とも読めるので弾く。
        let file = rule_with_window("06:00", "06:00");
        let err = validate_schedule(&file).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("窓つき"), "detail: {}", err.detail);
    }
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p casad --bin casad engine::tests::active_window_includes_from_and_excludes_to`
Expected: コンパイルエラー `cannot find function 'rule_is_active' in this scope`

- [ ] **Step 3: 最小の実装を書く**

3-a. `crates/casad/src/engine.rs:18` の use 文に `ActiveWindow` を足す:

```rust
use crate::rules::{parse_node_id, ActiveWindow, Rule, RuleFile, Trigger};
```

3-b. `parse_hm`（`crates/casad/src/engine.rs:37` の閉じ括弧）の直後に追加する:

```rust
/// ルールの有効時間帯を HH:MM 2 本として解析する。返り値は (from, to)。
/// 区間は `from` を含み `to` を含まない。`from > to` は日跨ぎを表す。
/// `from == to` は「空区間」とも「全日」とも読めるためエラーにする。
pub fn parse_active_window(w: &ActiveWindow) -> Result<(NaiveTime, NaiveTime), CasaError> {
    let from = parse_hm(&w.from)?;
    let to = parse_hm(&w.to)?;
    if from == to {
        return Err(CasaError::new(
            ErrorKind::ConfigParse,
            format!(
                "active window from and to are both \"{}\" (an empty window and an all-day window cannot be told apart)",
                w.from
            ),
        ));
    }
    Ok((from, to))
}

/// ルールが now の時点で有効か。`active` 未指定なら常に有効。
///
/// 解析できない窓は「無効」に倒す。起動前に [`validate_schedule`] が弾く前提だが、
/// 万一届いたら実機を動かさない側へ倒すのが安全側（不正な時刻の時刻トリガを
/// [`due_time_rules`] が発火させないのと同じ方針）。
fn rule_is_active(rule: &Rule, now: NaiveTime) -> bool {
    let Some(w) = &rule.active else {
        return true;
    };
    match parse_active_window(w) {
        Ok((from, to)) if from < to => now >= from && now < to,
        Ok((from, to)) => now >= from || now < to, // 日跨ぎ
        Err(_) => false,
    }
}
```

3-c. `validate_schedule` を書き換える:

```rust
/// すべての時刻トリガと有効時間帯が正しい HH:MM か検証する。
/// `casad check` / `run` の両方が使い、不正なルールで常駐が始まらないようにする。
pub fn validate_schedule(file: &RuleFile) -> Result<(), CasaError> {
    for rule in &file.rules {
        if let Trigger::Time { at } = &rule.when {
            parse_hm(at).map_err(|e| {
                CasaError::new(e.kind, format!("rule \"{}\": {}", rule.name, e.detail))
            })?;
        }
        if let Some(w) = &rule.active {
            parse_active_window(w).map_err(|e| {
                CasaError::new(e.kind, format!("rule \"{}\": {}", rule.name, e.detail))
            })?;
        }
    }
    Ok(())
}
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test -p casad --bin casad engine::`
Expected: 新規 5 件を含め全て PASS

- [ ] **Step 5: clippy を通す**

Run: `cargo clippy --workspace -- -D warnings`
Expected: warning なしで終了（exit 0）

- [ ] **Step 6: コミット**

```bash
git add crates/casad/src/engine.rs
git commit -m "feat(casad): active ウィンドウの解析と起動前検証

半開区間 [from, to) で判定し from > to は日跨ぎ。from == to は
全日とも空区間とも読めるので config_parse で弾く。"
```

---

### Task 3: 発火判定への適用と `--now` でのデバッグ経路

**Files:**
- Modify: `crates/casad/src/engine.rs`（`due_time_rules` / `due_event_rules` / `fire_due_events` / `drain_events_once` / `due_matter_event_rules` / `drain_matter_events_once` / `event_loop` / `matter_event_loop` / `run`）
- Modify: `crates/casad/src/cli.rs:45-62`（`Command::Run`）
- Modify: `crates/casad/src/main.rs:65-114`（`Command::Run` の処理）
- Test: `crates/casad/src/engine.rs` の `mod tests`、`crates/casad/tests/events.rs`

**Interfaces:**
- Consumes: `rule_is_active`（Task 2）
- Produces:
  - `pub fn now_or(override_now: Option<NaiveTime>) -> NaiveTime`
  - `pub fn fire_due_events(file, config, events, now: NaiveTime, config_path)`
  - `pub fn drain_events_once(file, config, enl_bin, now: NaiveTime, config_path)`
  - `pub fn drain_matter_events_once(file, config, mat_bin, now: NaiveTime, config_path)`
  - `--now` が `--once` / `--listen-once` / `--listen-once-mat` のいずれかと併用可能になる

- [ ] **Step 1: 失敗する単体テストを書く**

`crates/casad/src/engine.rs` の `mod tests` の末尾に追記する:

```rust
    #[test]
    fn due_time_rules_respects_active_window() {
        let file = rules(
            r#"
version = 1
[[rules]]
name = "窓外の時刻トリガ"
when = { at = "22:00" }
active = { from = "06:00", to = "21:00" }
then = { action = "off", device = "living_aircon" }
[[rules]]
name = "窓内の時刻トリガ"
when = { at = "12:00" }
active = { from = "06:00", to = "21:00" }
then = { action = "off", device = "living_aircon" }
"#,
        );
        // 22:00 は窓外なので、時刻が一致しても発火対象にならない。
        assert!(due_time_rules(&file, at(22, 0)).is_empty());
        let due = due_time_rules(&file, at(12, 0));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "窓内の時刻トリガ");
    }

    #[test]
    fn due_event_rules_respects_active_window() {
        let file = rules(
            r#"
version = 1
[[rules]]
name = "昼だけ電源ONで点灯"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
active = { from = "06:00", to = "21:00" }
then = { action = "on", device = "living_aircon" }
"#,
        );
        let cfg = config_living();
        let inside = due_event_rules(
            &file,
            &cfg,
            &[event("192.0.2.10", "013001", "80", "30")],
            at(12, 0),
        );
        assert_eq!(inside.len(), 1);
        let outside = due_event_rules(
            &file,
            &cfg,
            &[event("192.0.2.10", "013001", "80", "30")],
            at(3, 0),
        );
        assert!(outside.is_empty());
    }

    #[test]
    fn due_matter_event_rules_respects_active_window() {
        let file = rules(
            r#"
version = 1
[[rules]]
name = "書斎 不在で扇風機ON"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
active = { from = "06:00", to = "21:00" }
then = { action = "on", device = "desk_tape_light" }
"#,
        );
        let cfg = config_matter();
        let inside = due_matter_event_rules(
            &file,
            &cfg,
            &[mat_event(16, 1, "occupancy", serde_json::json!(0))],
            at(12, 0),
        );
        assert_eq!(inside.len(), 1);
        let outside = due_matter_event_rules(
            &file,
            &cfg,
            &[mat_event(16, 1, "occupancy", serde_json::json!(0))],
            at(21, 30),
        );
        assert!(outside.is_empty());
    }

    #[test]
    fn windowless_rules_still_fire_at_any_time() {
        // 後方互換: active を持たないルールは従来どおりどの時刻でも発火する。
        let file = rules(SCHEDULE);
        assert_eq!(due_time_rules(&file, at(22, 0)).len(), 1);
        let cfg = config_living();
        assert_eq!(
            due_event_rules(
                &file,
                &cfg,
                &[event("192.0.2.10", "013001", "80", "30")],
                at(3, 0)
            )
            .len(),
            1
        );
    }
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p casad --bin casad engine::tests::due_time_rules_respects_active_window`
Expected: 引数の数が合わずコンパイルエラー（`due_event_rules` / `due_matter_event_rules` が 3 引数）。`due_time_rules_respects_active_window` 自体は「窓を見ていない」ため assert 失敗になる。

- [ ] **Step 3: マッチ関数に窓を適用する**

3-a. `rule_is_active` の直後に窓外ログのヘルパを足す:

```rust
/// 窓外で落としたルールを debug ログに残す。「センサーは反応しているのにルールが
/// 動かない」の切り分けコストを下げるため。**トリガに一致した後にだけ**呼ぶこと
/// （毎イベントで全ルール分のログを出さない）。
fn active_or_log(rule: &Rule, now: NaiveTime) -> bool {
    if rule_is_active(rule, now) {
        return true;
    }
    tracing::debug!(rule = %rule.name, %now, "rule skipped: outside its active window");
    false
}
```

3-b. `due_time_rules` の `.filter(...)` チェーンの末尾（`.collect()` の直前）に 1 行足す:

```rust
        .filter(|r| active_or_log(r, now))
        .collect()
```

3-c. `due_event_rules` を書き換える:

```rust
fn due_event_rules<'a>(
    file: &'a RuleFile,
    config: &Config,
    events: &[enl::Event],
    now: NaiveTime,
) -> Vec<&'a Rule> {
    file.rules
        .iter()
        .filter(|r| matches!(r.when, Trigger::Event { .. }))
        .filter(|r| events.iter().any(|e| event_matches(r, config, e)))
        .filter(|r| active_or_log(r, now))
        .collect()
}
```

3-d. `fire_due_events` と `drain_events_once` を書き換える:

```rust
pub fn fire_due_events(
    file: &RuleFile,
    config: &Config,
    events: &[enl::Event],
    now: NaiveTime,
    config_path: Option<&Path>,
) -> usize {
    fire_all(due_event_rules(file, config, events, now), config_path)
}

pub fn drain_events_once(
    file: &RuleFile,
    config: &Config,
    enl_bin: &str,
    now: Option<NaiveTime>,
    config_path: Option<&Path>,
) -> Result<usize, CasaError> {
    let events = enl::listen_once(enl_bin)?;
    // 窓判定は listen が返った後の時刻で行う（listen は何時間もブロックしうる）。
    // --now が与えられていればそれを優先する（デバッグ用の固定時刻）。
    Ok(fire_due_events(file, config, &events, now_or(now), config_path))
}
```

3-e. `due_matter_event_rules` と `drain_matter_events_once` を書き換える:

```rust
fn due_matter_event_rules<'a>(
    file: &'a RuleFile,
    config: &Config,
    events: &[mat::Event],
    now: NaiveTime,
) -> Vec<&'a Rule> {
    file.rules
        .iter()
        .filter(|r| matches!(r.when, Trigger::MatterEvent { .. }))
        .filter(|r| events.iter().any(|e| matter_event_matches(r, config, e)))
        .filter(|r| active_or_log(r, now))
        .collect()
}

pub fn drain_matter_events_once(
    file: &RuleFile,
    config: &Config,
    mat_bin: &str,
    now: Option<NaiveTime>,
    config_path: Option<&Path>,
) -> Result<usize, CasaError> {
    let events = mat::listen_once(mat_bin)?;
    // 同上。listen が返った後の時刻で窓を判定する（--now があればそれを優先）。
    Ok(fire_all(
        due_matter_event_rules(file, config, &events, now_or(now)),
        config_path,
    ))
}
```

3-f. `event_loop` / `matter_event_loop` の `Ok(events) => {` ブロック冒頭で時刻を取る。listen は何時間もブロックしうるので、**listen が返った後**に評価するのが要点:

```rust
            Ok(events) => {
                // 窓判定は listen が返った後の時刻で行う（listen は何時間もブロックしうる）。
                let now = Local::now().time();
                let queued = dispatcher.dispatch_all(due_event_rules(file, config, &events, now));
```

```rust
            Ok(events) => {
                // 同上。listen が返った時刻で窓を判定する。
                let now = Local::now().time();
                let queued =
                    dispatcher.dispatch_all(due_matter_event_rules(file, config, &events, now));
```

3-g. `run()` の `--once` 分岐で使えるよう、`parse_active_window` の直後に `now_or` を足す:

```rust
/// `--now` の上書きを解決する。None なら実時計（ローカルタイム）。
pub fn now_or(override_now: Option<NaiveTime>) -> NaiveTime {
    override_now.unwrap_or_else(|| Local::now().time())
}
```

3-h. `run()` の `if opts.once {` ブロック 1 行目を書き換える:

```rust
        let now = now_or(opts.now);
```

- [ ] **Step 4: 単体テストが通ることを確認**

Run: `cargo test -p casad --bin casad engine::`
Expected: 新規 4 件を含め全て PASS

- [ ] **Step 5: `--now` の併用制約を緩める**

`crates/casad/src/cli.rs` の `Run` バリアントを書き換える（`#[command(group = ...)]` を追加し、`now` の `requires` を差し替える）:

```rust
    /// ルールエンジンを起動する。既定は常駐し、時刻トリガ（毎分の境界で評価）と
    /// イベントトリガ（enl listen を回して状変通知に反応）を並行に走らせる。
    #[command(group = clap::ArgGroup::new("oneshot").args(["once", "listen_once", "listen_once_mat"]))]
    Run {
        /// ルールファイル（rules.toml）のパス
        rules: PathBuf,
        /// 時刻トリガを 1 回だけ評価して終了する（cron 毎分起動、またはデバッグ用）。
        #[arg(long)]
        once: bool,
        /// 現在時刻を HH:MM で上書きする（1 回だけ評価する 3 つの経路と併用するデバッグ用）。
        /// 時刻トリガの評価と、ルールの有効時間帯（active）判定の両方に効く。
        #[arg(long, value_name = "HH:MM", requires = "oneshot")]
        now: Option<String>,
        /// enl listen を 1 回だけ起動し、得た通知でイベントトリガを評価して終了する
        /// （デバッグ用。通知が来るまでブロックする）。
        #[arg(long, conflicts_with = "once")]
        listen_once: bool,
        /// mat listen を 1 回だけ起動し、得たイベントで Matter トリガを評価して終了する
        /// （デバッグ用。イベントが来るまでブロックする）。
        #[arg(long, conflicts_with_all = ["once", "listen_once"])]
        listen_once_mat: bool,
    },
```

`crates/casad/src/main.rs` の `Command::Run` 分岐を書き換える。`--now` の解析を `listen_once` 分岐より**前**に移し、3 経路すべてに渡す:

```rust
            // 子 CLI は casa と同じ規約（CASA_<BIN>_BIN / [binaries] / PATH）で解決する。
            let enl_bin = casa_core::runner::resolve_bin("enl", &config);
            let mat_bin = casa_core::runner::resolve_bin("mat", &config);

            // --now は 1 回だけ評価する 3 経路で共通に効く。時刻トリガの評価と
            // ルールの有効時間帯（active）判定の両方に使う。
            let now = now.map(|s| engine::parse_hm(&s)).transpose()?;

            if listen_once {
                // イベント側のデバッグ経路: enl listen を 1 回回して評価し終了する。
                let fired = engine::drain_events_once(
                    &rule_file,
                    &config,
                    &enl_bin,
                    now,
                    cli.config.as_deref(),
                )?;
                tracing::info!(fired, "single event drain complete");
                return Ok(0);
            }
            if listen_once_mat {
                // Matter イベント側のデバッグ経路: mat listen を 1 回回して評価し終了する。
                let fired = engine::drain_matter_events_once(
                    &rule_file,
                    &config,
                    &mat_bin,
                    now,
                    cli.config.as_deref(),
                )?;
                tracing::info!(fired, "single matter event drain complete");
                return Ok(0);
            }

            engine::run(
                &rule_file,
                &config,
                cli.config.as_deref(),
                &enl_bin,
                &mat_bin,
                engine::RunOpts { once, now },
            )
```

（元の `let now = now.map(|s| engine::parse_hm(&s)).transpose()?;` の行は削除する。移動先で 1 回だけ解析する。）

- [ ] **Step 6: 統合テストを書く**

`crates/casad/tests/events.rs` の末尾に追記する:

```rust
const WINDOWED_MATTER_RULES: &str = r#"
version = 1
[[rules]]
name = "書斎 不在で消灯（昼だけ）"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
active = { from = "06:00", to = "21:00" }
then = { action = "off", device = "desk_tape_light" }
"#;

#[test]
fn listen_once_mat_fires_inside_active_window() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);
    let rules = write_rules(dir.path(), WINDOWED_MATTER_RULES);

    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once-mat",
            "--now",
            "12:00",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_MAT_BIN", &fixture("mat_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
        ],
    );

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("off desk_tape_light"), "stdout: {stdout}");
}

#[test]
fn listen_once_mat_does_not_fire_outside_active_window() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);
    let rules = write_rules(dir.path(), WINDOWED_MATTER_RULES);

    // 22:00 は 06:00-21:00 の窓の外。イベントが一致しても発火しない。
    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once-mat",
            "--now",
            "22:00",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_MAT_BIN", &fixture("mat_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
        ],
    );

    assert_eq!(out.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("casa called"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn now_without_a_oneshot_flag_is_a_cli_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);
    let rules = write_rules(dir.path(), WINDOWED_MATTER_RULES);

    // --now は 1 回だけ評価する経路専用。常駐起動に付けても黙って無視しない。
    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--now",
            "12:00",
            "--config",
            config.to_str().unwrap(),
        ],
        &[],
    );

    assert_eq!(out.status.code(), Some(2));
}
```

- [ ] **Step 7: 全テストが通ることを確認**

Run: `cargo test -p casad`
Expected: 全 PASS

`now_without_a_oneshot_flag_is_a_cli_error` が exit 2 にならない場合、clap の `ArgGroup` 経由の `requires` が効いていない。その場合の代替は main.rs 側の明示チェック:

```rust
            if now.is_some() && !once && !listen_once && !listen_once_mat {
                // clap 既定の引数エラーと同じ exit 2 に揃える。
                return Err(CasaError::new(
                    ErrorKind::ConfigParse,
                    "--now requires one of --once / --listen-once / --listen-once-mat".to_string(),
                ));
            }
```

ただし `ErrorKind::ConfigParse` は exit 10 になるため、この代替を採る場合はテストの期待値を `Some(10)` に変え、`--now` が無視されないことの検証に主眼を置くこと。まず `ArgGroup` で exit 2 を狙う。

- [ ] **Step 8: clippy を通す**

Run: `cargo clippy --workspace -- -D warnings`
Expected: warning なしで終了（exit 0）

- [ ] **Step 9: コミット**

```bash
git add crates/casad/src/engine.rs crates/casad/src/cli.rs crates/casad/src/main.rs crates/casad/tests/events.rs
git commit -m "feat(casad): active ウィンドウを発火判定に適用

時刻/echonet/matter の 3 マッチ関数で窓外のルールを落とし、落とした
ことを debug ログに残す。--now を --listen-once(-mat) でも使えるように
して、実時刻を待たずに窓の挙動を検証できるようにした。"
```

---

### Task 4: `casad check` の統合テストと examples

**Files:**
- Test: `crates/casad/tests/check.rs`
- Modify: `examples/rules.toml`

**Interfaces:**
- Consumes: `Rule.active` の serde 表現（Task 1）、`validate_schedule` の `active` 検証（Task 2）
- Produces: なし（検証とサンプルのみ）

- [ ] **Step 1: 失敗するテストを書く**

`crates/casad/tests/check.rs` の末尾に追記する:

```rust
#[test]
fn check_reports_active_window_and_omits_it_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(
        dir.path(),
        r#"
version = 1
[[rules]]
name = "常時"
when = { at = "22:00" }
then = { action = "off", device = "living_aircon" }
[[rules]]
name = "昼だけ"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
active = { from = "06:00", to = "21:00" }
then = { action = "on", device = "living_aircon" }
"#,
    );

    let out = run_casad(
        &[
            "check",
            rules.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
        ],
        &[],
    );

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    // active を持たないルールの JSON は従来どおり（後方互換）。
    assert!(v["rules"][0].get("active").is_none());
    assert_eq!(v["rules"][1]["active"]["from"], "06:00");
    assert_eq!(v["rules"][1]["active"]["to"], "21:00");
}

#[test]
fn check_zero_width_active_window_exits_10() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(
        dir.path(),
        r#"
version = 1
[[rules]]
name = "幅ゼロの窓"
when = { at = "22:00" }
active = { from = "06:00", to = "06:00" }
then = { action = "off", device = "living_aircon" }
"#,
    );

    let out = run_casad(
        &[
            "check",
            rules.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
        ],
        &[],
    );

    assert_eq!(out.status.code(), Some(10));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "config_parse");
}

#[test]
fn check_malformed_active_window_exits_10() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), DUMMY_CONFIG);
    let rules = write_rules(
        dir.path(),
        r#"
version = 1
[[rules]]
name = "壊れた窓"
when = { at = "22:00" }
active = { from = "6am", to = "21:00" }
then = { action = "off", device = "living_aircon" }
"#,
    );

    let out = run_casad(
        &[
            "check",
            rules.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
        ],
        &[],
    );

    assert_eq!(out.status.code(), Some(10));
    assert_eq!(stderr_error_json(&out)["error"]["kind"], "config_parse");
}
```

- [ ] **Step 2: テストが失敗しないことを確認（Task 1-3 が正しければ最初から通る）**

Run: `cargo test -p casad --test check`
Expected: 全 PASS。落ちる場合は Task 1 の `skip_serializing_if` か Task 2 の `validate_schedule` に漏れがある。

- [ ] **Step 3: examples/rules.toml にサンプルを追加**

`examples/rules.toml` の末尾に追記する:

```toml
# Active window: this rule is only in effect between 06:00 and 21:00. The start is
# inclusive and the end is exclusive, so it is already inactive at 21:00 sharp.
# `from` later than `to` wraps over midnight (e.g. from = "21:00", to = "06:00").
# Omit `active` entirely and the rule is in effect at all times.
[[rules]]
name = "example rule limited to daytime"
when = { device = "living_light", attribute = "onoff", equals = false }
active = { from = "06:00", to = "21:00" }
then = { action = "on", device = "bedroom_light" }
```

- [ ] **Step 4: examples が検証を通ることを確認**

Run: `cargo test -p casad --test check check_example_rules_validate_against_example_config`
Expected: PASS（既存テストが `examples/rules.toml` を `casad check` に通している）

- [ ] **Step 5: 全テストと clippy**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: 全 PASS、warning なし

- [ ] **Step 6: コミット**

```bash
git add crates/casad/tests/check.rs examples/rules.toml
git commit -m "test(casad): active ウィンドウの check 統合テストとサンプルを追加"
```

---

### Task 5: ドキュメントとバージョン bump

**Files:**
- Modify: `README.md`（casad セクション）
- Modify: `CLAUDE.md`（casad の責務の記述）
- Modify: `Cargo.toml:6`（workspace version）

**Interfaces:**
- Consumes: Task 1-4 で確定した DSL 表面
- Produces: なし

- [ ] **Step 1: README にサンプルと説明を追加**

`README.md` の casad セクション、rules.toml のサンプルコードブロック（` ```toml ` 〜 ` ``` `）の末尾、`# Multiple actions:` のルールの後ろに追記する:

```toml
# Active window: this rule only fires between 06:00 and 21:00.
[[rules]]
name = "circulate air in the study while nobody is there"
when   = { device = "study_motion", attribute = "occupancy", equals = 0 }
active = { from = "06:00", to = "21:00" }
then   = { action = "on", device = "study_fan" }
```

続けて、`` `then` accepts either a single table or an array of them. `` で始まる段落の**直後**に新しい段落を追加する:

```markdown
`active = { from = "HH:MM", to = "HH:MM" }` limits a rule to a time window. The
window is half-open: `from` is included and `to` is excluded, so a rule with
`to = "21:00"` is already inactive at 21:00 sharp. A `from` later than `to`
wraps over midnight (`{ from = "21:00", to = "06:00" }` covers 21:00–23:59 and
00:00–05:59). `from` equal to `to` is a config error, because an empty window
and an all-day window cannot be told apart. Omit `active` and the rule is in
effect at all times. The window applies to every trigger kind — time, ECHONET
event, and Matter event — and is evaluated against local time when the trigger
matches. A rule dropped because it is outside its window is logged at debug
level.
```

続けて、` casad run rules.toml --once --now 22:00 ` を含むコードブロックの `--listen-once-mat` の行の後ろに追記する:

```bash
# Debug: evaluate as if it were 22:00 (works with --once / --listen-once / --listen-once-mat).
# Drives both time-trigger evaluation and the `active` window check.
casad run rules.toml --listen-once-mat --now 22:00
```

- [ ] **Step 2: CLAUDE.md の casad 責務に追記**

`CLAUDE.md` の「Evaluation engine for the automation rule DSL (`rules.toml`) — **implemented**:」の箇条書きのうち、`Firing is dispatched asynchronously...` の項目の**前**に 1 項目を挿入する:

```markdown
  - Per-rule active window (`active = { from = "HH:MM", to = "HH:MM" }`): a rule only fires
    inside its window. Half-open (`from` inclusive, `to` exclusive), `from` > `to` wraps over
    midnight, `from` == `to` is a config error, and omitting it means always in effect. It
    applies to every trigger kind.
```

- [ ] **Step 3: バージョンを上げる**

`Cargo.toml:6` を書き換える:

```toml
version = "1.4.0"
```

- [ ] **Step 4: ビルドとテストを通す**

Run: `cargo build --workspace && cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: 全 PASS、warning なし（`Cargo.lock` の version が更新される）

- [ ] **Step 5: バージョンが反映されたことを確認**

Run: `cargo run -p casad -- --version`
Expected: `casad 1.4.0`

- [ ] **Step 6: コミット**

```bash
git add README.md CLAUDE.md Cargo.toml Cargo.lock
git commit -m "docs: active ウィンドウを文書化し 1.4.0 に bump"
```

---

### Task 6: jarvis への配布と実機検証

**Files:**
- Modify: `~/ghq/github.com/nogu3/jarvis-iac/roles/casa/files/devices.toml`
- Modify: `~/ghq/github.com/nogu3/jarvis-iac/roles/casa/files/rules.toml`

**Interfaces:**
- Consumes: `casad` 1.4.0 バイナリ（Task 5）、`active` の TOML 表面（Task 1）
- Produces: 稼働する実機ルール

**順序の要点:** **バイナリを先、設定を後**。serde は未知フィールドを既定で無視するため、
旧 casad に `active` 付き rules.toml を配ると窓が黙って無視され、扇風機が 24 時間
人感連動してしまう。必ずこの順で行う。

- [ ] **Step 1: aarch64 へクロスビルド**

```bash
cd ~/ghq/github.com/nogu3/casa
cross build --release --target aarch64-unknown-linux-musl -p casad
```
Expected: `target/aarch64-unknown-linux-musl/release/casad` が生成される

- [ ] **Step 2: jarvis へ転送**

```bash
scp target/aarch64-unknown-linux-musl/release/casad jarvis:~/.local/bin/casad.new
```
Expected: 転送完了（exit 0）

- [ ] **Step 3: アトミックに差し替える（まだ restart しない）**

対象ホスト `jarvis` / バイナリ `~/.local/bin/casad` / unit `casad`（user）を復唱してから実行する。

```bash
ssh jarvis 'install -m755 ~/.local/bin/casad.new ~/.local/bin/casad && rm -f ~/.local/bin/casad.new && ~/.local/bin/casad --version'
```
Expected: `casad 1.4.0`（差し替えただけなので、稼働中の casad はまだ旧バイナリのまま）

- [ ] **Step 4: restart する前に、新バイナリで本番ルールを検証する**

新設した「時刻トリガの `at` が自分の `active` 窓の外」検証は、1 件でも該当すると
`validate_schedule` がそこで返り、**`casad run` が起動時に exit 10 で落ちてルールが全部止まる**。
restart してから気づくと照明もファンも自動化が死ぬので、restart の前に必ず通す。

```bash
ssh jarvis '~/.local/bin/casad check ~/.config/casa/rules.toml --config ~/.config/casa/devices.toml'; echo "exit=$?"
```
Expected: `exit=0` と `"ok":true`。exit 10 が出たらエラーが名指ししたルールを直してから進む
（この時点ではまだ旧 casad が動き続けているので、実害なく引き返せる）

- [ ] **Step 5: サービスを再起動する**

```bash
ssh jarvis 'systemctl --user restart casad && systemctl --user status casad --no-pager --lines=0'
```
Expected: `Active: active (running)`

- [ ] **Step 6: jarvis-iac に扇風機デバイスを追加**

`~/ghq/github.com/nogu3/jarvis-iac/roles/casa/files/devices.toml` の末尾（`[devices.plant_light]` の後ろ）に追記する:

```toml
# 書斎の扇風機。Hub 2 に赤外線リモコン "扇風機(BARREL)" (DIY Fan) として登録。
# 在室中と 21 時以降は音がうるさいので、casad が人感センサーで昼間だけ回す。
[devices.study_fan]
protocol = "switchbot"
device_id = "01-202607251040-40751692"
```

- [ ] **Step 7: jarvis-iac にルールを追加**

`~/ghq/github.com/nogu3/jarvis-iac/roles/casa/files/rules.toml` の末尾に追記する。
既存の「書斎 人感OFFで消灯 / 人感ONで点灯」は**変更しない**（照明は夜間も人感で動かす）。

```toml
# 書斎に人がいない間だけ扇風機で空気を回す。在室中は音がうるさいので止める。
# 21 時以降は就寝のため連動そのものを止める（active ウィンドウ）。
[[rules]]
name   = "書斎 不在で扇風機ON"
when   = { device = "study_motion", attribute = "occupancy", equals = 0 }
active = { from = "06:00", to = "21:00" }
then   = { action = "on", device = "study_fan" }

[[rules]]
name   = "書斎 在室で扇風機OFF"
when   = { device = "study_motion", attribute = "occupancy", equals = 1 }
active = { from = "06:00", to = "21:00" }
then   = { action = "off", device = "study_fan" }

# 21 時に無条件で止める。赤外線なので状態は読めないが、ON/OFF が別コードなので
# 既に止まっているときに OFF を送っても何も起きない。
[[rules]]
name = "扇風機 21時消灯"
when = { at = "21:00" }
then = { action = "off", device = "study_fan" }
```

- [ ] **Step 8: Ansible の差分を確認**

```bash
cd ~/ghq/github.com/nogu3/jarvis-iac
export PATH=$HOME/.local/bin:$PATH
ansible-playbook site.yml --check --diff
```
Expected: `devices.toml` と `rules.toml` の 2 ファイルのみ changed。他が changed なら drift なので先に調査する。

- [ ] **Step 9: 本適用**

```bash
cd ~/ghq/github.com/nogu3/jarvis-iac
export PATH=$HOME/.local/bin:$PATH
ansible-playbook site.yml
```
Expected: 上記 2 ファイルが changed、handler `Restart casad (casa config)` が走る

- [ ] **Step 10: ルールが受理されたことを確認**

```bash
ssh jarvis '~/.local/bin/casad check ~/.config/casa/rules.toml' | python3 -m json.tool | head -20
ssh jarvis 'systemctl --user status casad --no-pager --lines=5'
```
Expected: `"ok": true` と増えたルール数、casad が `active (running)`

- [ ] **Step 11: 窓外では発火しないことを実機で確認**

```bash
ssh jarvis 'export PATH=$HOME/.local/bin:$PATH; RUST_LOG=debug casad run ~/.config/casa/rules.toml --listen-once-mat --now 22:00 2>&1 | tail -20'
```
書斎に入る／出るなどして人感イベントを起こす。
Expected: 扇風機のルールは `rule skipped: outside its active window` の debug ログが出るだけで発火せず、既存の照明ルールは発火する（扇風機は回らない）

- [ ] **Step 12: 窓内では発火することを実機で確認**

```bash
ssh jarvis 'export PATH=$HOME/.local/bin:$PATH; RUST_LOG=debug casad run ~/.config/casa/rules.toml --listen-once-mat --now 12:00 2>&1 | tail -20'
```
書斎から出て不在イベントを起こす。
Expected: `firing rule` に `書斎 不在で扇風機ON` が出て、**実機の扇風機が回り出す**

- [ ] **Step 13: 21 時の停止を実機で確認**

```bash
ssh jarvis 'export PATH=$HOME/.local/bin:$PATH; RUST_LOG=debug casad run ~/.config/casa/rules.toml --once --now 21:00 2>&1 | tail -20'
```
Expected: `firing rule` に `扇風機 21時消灯` が出て、**回っている扇風機が止まる**。
止まっている状態でもう一度実行しても回り出さない（ON/OFF が別コードであることの確認）

- [ ] **Step 14: jarvis-iac をコミット**

```bash
cd ~/ghq/github.com/nogu3/jarvis-iac
git add roles/casa/files/devices.toml roles/casa/files/rules.toml
git commit -m "feat(casa): 書斎の扇風機を人感連動(6-21時)で追加

在室中と 21 時以降は音がうるさいので、casad の active ウィンドウで
昼間だけ人感連動させ、21 時に無条件で止める。"
```

- [ ] **Step 15: drift が消えたことを確認**

```bash
cd ~/ghq/github.com/nogu3/jarvis-iac
export PATH=$HOME/.local/bin:$PATH
ansible-playbook site.yml --check --diff
```
Expected: `changed=0`
