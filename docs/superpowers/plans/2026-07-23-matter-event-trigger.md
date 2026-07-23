# casad Matter イベントトリガ対応 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** casad が `mat listen`(matd 常駐 Subscribe の薄いクライアント)経由で Matter デバイスの状変を受け、ルールを発火できるようにする。最初のユースケース: 書斎人感センサー(node 16, occupancy)off→`desk_tape_light` 消灯 / on→点灯。

**Architecture:** enl と対称の one-shot ループ方式。casad が `mat listen --count 1 --timeout-ms 0` を繰り返し起動し、1 行 1 JSON のイベントをパースしてルールと突合、既存 Dispatcher に積む。`priming: true`(matd 再購読時の現在値再配達)は無条件で捨てる。spec: `docs/superpowers/specs/2026-07-23-matter-event-trigger-design.md`

**Tech Stack:** Rust / clap(derive) / serde + serde_json / toml / tracing。新規依存なし。

## Global Constraints

- casad の子プロセス結合は stdout JSON スキーマのみ(matd socket への直結は禁止)
- mat バイナリ解決は `CASA_MAT_BIN` / `[binaries]` / PATH(casa-core `runner::resolve_bin("mat", &config)`)
- CI に実 mat / matd は不要(fixture スクリプトで代役)
- 検証コマンド: `cargo build && cargo test && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check`
- コミットは自分が編集したファイルのみ `git add`(ユーザー CLAUDE.md 規約)
- コミットメッセージ末尾に Co-Authored-By / Claude-Session トレーラを付ける(セッション規約どおり)

---

### Task 1: rules.rs — Matter イベントトリガの DSL

**Files:**
- Modify: `crates/casad/src/rules.rs`

**Interfaces:**
- Produces: `Trigger::MatterEvent { device: String, attribute: String, equals: serde_json::Value }`(untagged variant、`attribute` キーで判別)
- Produces: `pub(crate) fn parse_node_id(s: &str) -> Option<u64>`(10 進文字列のみ。engine の突合と validate の両方が使う)
- Produces: `RuleFile::validate` が Matter トリガの device に対し「存在 + `protocol = "matter"` + node_id が数値」を検証(protocol 不一致は `ErrorKind::ProtocolUnsupported`)

- [ ] **Step 1: 失敗するテストを書く**

`crates/casad/src/rules.rs` の `mod tests` に追加:

```rust
    #[test]
    fn parses_matter_event_rule() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "書斎 人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        match &file.rules[0].when {
            Trigger::MatterEvent {
                device,
                attribute,
                equals,
            } => {
                assert_eq!(device, "study_motion");
                assert_eq!(attribute, "occupancy");
                assert_eq!(equals, &serde_json::json!(0));
            }
            other => panic!("unexpected trigger: {other:?}"),
        }
    }

    #[test]
    fn matter_equals_accepts_bool_and_string() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "bool"
when = { device = "d", attribute = "onoff", equals = true }
then = { action = "on", device = "d" }
[[rules]]
name = "string"
when = { device = "d", attribute = "mode", equals = "auto" }
then = { action = "on", device = "d" }
"#,
        )
        .unwrap();
        match &file.rules[0].when {
            Trigger::MatterEvent { equals, .. } => assert_eq!(equals, &serde_json::json!(true)),
            other => panic!("unexpected trigger: {other:?}"),
        }
        match &file.rules[1].when {
            Trigger::MatterEvent { equals, .. } => assert_eq!(equals, &serde_json::json!("auto")),
            other => panic!("unexpected trigger: {other:?}"),
        }
    }

    #[test]
    fn echonet_event_rule_still_parses_as_event() {
        // epc キーは従来どおり Event variant に落ちる（MatterEvent に吸われない）。
        let file = parse(VALID).unwrap();
        assert!(matches!(file.rules[0].when, Trigger::Event { .. }));
    }

    #[test]
    fn parse_node_id_accepts_decimal_only() {
        assert_eq!(parse_node_id("16"), Some(16));
        assert_eq!(parse_node_id(" 16 "), Some(16));
        assert_eq!(parse_node_id("0x10"), None);
        assert_eq!(parse_node_id("study_motion"), None);
    }

    fn config_with_matter() -> Config {
        casa_core::config::parse(
            r#"
version = 1
[devices.study_motion]
protocol = "matter"
node_id = "16"
[devices.desk_tape_light]
protocol = "matter"
node_id = "6"
[devices.washstand_light]
protocol = "echonet"
ip = "192.0.2.11"
eoj = "0x029101"
[devices.alias_node]
protocol = "matter"
node_id = "not_a_number"
"#,
        )
        .unwrap()
    }

    #[test]
    fn validate_accepts_matter_event_rule() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        file.validate(&config_with_matter()).unwrap();
    }

    #[test]
    fn validate_rejects_matter_trigger_on_echonet_device() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "プロトコル不一致"
when = { device = "washstand_light", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        let err = file.validate(&config_with_matter()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ProtocolUnsupported);
        assert!(err.detail.contains("プロトコル不一致"));
    }

    #[test]
    fn validate_rejects_non_numeric_node_id_for_matter_trigger() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "数値でないnode_id"
when = { device = "alias_node", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        let err = file.validate(&config_with_matter()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("数値でないnode_id"));
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p casad --lib rules`
Expected: FAIL(`MatterEvent` variant 未定義・`parse_node_id` 未定義のコンパイルエラー)

- [ ] **Step 3: 最小実装を書く**

`Trigger` enum に variant を追加(`Event` の後ろ):

```rust
/// トリガ。TOML ではインラインテーブルで、含まれるキーで種別が決まる（untagged）。
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Trigger {
    /// イベント: あるデバイスの EPC が指定値になったとき。
    /// 例: `when = { device = "entry_motion", epc = "0x80", equals = "0x30" }`
    Event {
        device: String,
        epc: String,
        equals: String,
    },
    /// Matter イベント: あるデバイスの属性が指定値になったとき（mat listen 経由）。
    /// 例: `when = { device = "study_motion", attribute = "occupancy", equals = 0 }`
    /// equals は matd イベントの JSON `value` と等値比較する（数値 / bool / 文字列）。
    MatterEvent {
        device: String,
        attribute: String,
        equals: serde_json::Value,
    },
    /// 時刻: 毎日その時刻（HH:MM）になったとき。
    /// 例: `when = { at = "22:00" }`
    Time { at: String },
}
```

`parse_node_id` を追加(validate と engine の突合が共用):

```rust
/// devices.toml の `node_id`（文字列）を数値へ。mat listen イベントの
/// node_id（数値）との突合に使う。10 進のみ（mat の alias は不可）。
pub(crate) fn parse_node_id(s: &str) -> Option<u64> {
    s.trim().parse().ok()
}
```

`RuleFile::validate` の Event 分岐の下に MatterEvent 分岐を追加:

```rust
    pub fn validate(&self, config: &Config) -> Result<(), CasaError> {
        for rule in &self.rules {
            match &rule.when {
                Trigger::Event { device, .. } => {
                    check_device(config, &rule.name, device)?;
                }
                Trigger::MatterEvent { device, .. } => {
                    check_matter_device(config, &rule.name, device)?;
                }
                Trigger::Time { .. } => {}
            }
            check_target(config, &rule.name, rule.then.device())?;
        }
        Ok(())
    }
```

`check_device` の下にヘルパを追加:

```rust
/// Matter イベントトリガの発火元検証: 存在 + protocol=matter + node_id が数値。
/// mat listen の通知と node_id で突き合わせるため、数値化できない設定は起動前に弾く。
fn check_matter_device(config: &Config, rule_name: &str, device: &str) -> Result<(), CasaError> {
    let dev = config
        .device(device)
        .map_err(|e| CasaError::new(e.kind, format!("rule \"{rule_name}\": {}", e.detail)))?;
    match dev {
        casa_core::config::Device::Matter { node_id, .. } => {
            if parse_node_id(node_id).is_none() {
                return Err(CasaError::new(
                    ErrorKind::ConfigParse,
                    format!(
                        "rule \"{rule_name}\": device \"{device}\" node_id \"{node_id}\" is not numeric"
                    ),
                ));
            }
            Ok(())
        }
        other => Err(CasaError::new(
            ErrorKind::ProtocolUnsupported,
            format!(
                "rule \"{rule_name}\": matter event trigger requires a matter device, but \"{device}\" is {}",
                other.protocol()
            ),
        )),
    }
}
```

注意: engine.rs の既存 `event_matches` / `due_time_rules` の match は `Trigger::Event` / `Trigger::Time` を個別に見ているので、`else return false` / `_ => false` 型のパターンならコンパイルは通る。網羅 match でエラーになる箇所があれば `Trigger::MatterEvent { .. } => false`(または `()`)の腕を足す。

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p casad`
Expected: 全 PASS(既存テスト含む)

- [ ] **Step 5: コミット**

```bash
git add crates/casad/src/rules.rs crates/casad/src/engine.rs
git commit -m "feat(casad): ルール DSL に Matter イベントトリガを追加"
```

---

### Task 2: mat.rs — mat listen の起動とパース

**Files:**
- Create: `crates/casad/src/mat.rs`
- Modify: `crates/casad/src/main.rs`(`mod mat;` 追加のみ)

**Interfaces:**
- Consumes: `casa_core::error::{CasaError, ErrorKind}`
- Produces: `pub struct Event { pub node_id: u64, pub endpoint: u64, pub cluster: serde_json::Value, pub attribute: serde_json::Value, pub value: serde_json::Value, pub priming: bool }`
- Produces: `pub fn listen_once(bin: &str) -> Result<Vec<Event>, CasaError>`
- Produces: `fn parse_lines(stdout: &[u8]) -> Result<Vec<Event>, CasaError>`(listen_once 内部・テスト対象)

- [ ] **Step 1: 失敗するテストを含めてファイルを作る**

`crates/casad/src/mat.rs` を新規作成:

```rust
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
```

`crates/casad/src/main.rs` の `mod enl;` の下に `mod mat;` を追加。

- [ ] **Step 2: テストが通ることを確認する**

Run: `cargo test -p casad --lib mat`
Expected: 3 テスト PASS(`listen_once` が未使用の dead_code warning が出る場合は次 Task で使われるため `cargo clippy` が落ちるか確認し、落ちるなら一時的に `#[allow(dead_code)]` を付けず Task 3 まで同一コミットにせず、`pub` アイテムは bin クレートでも到達可能性で警告されない — casad は bin なので未使用 pub は dead_code になる。**警告が出たら Task 3 完了までは `git commit` を Task 3 とまとめず、ここでは commit せずに Task 3 に進んでよい**。ただし原則はタスク毎コミットなので、警告が出ない場合はここでコミットする)

- [ ] **Step 3: clippy 確認**

Run: `cargo clippy --workspace -- -D warnings`
Expected: dead_code で FAIL する場合は Task 3 を先に実装してからまとめてコミット。PASS ならここでコミット。

- [ ] **Step 4: コミット(clippy PASS の場合のみ。FAIL なら Task 3 と合流)**

```bash
git add crates/casad/src/mat.rs crates/casad/src/main.rs
git commit -m "feat(casad): mat listen の起動・パースモジュールを追加"
```

---

### Task 3: engine.rs — Matter イベントの突合と発火

**Files:**
- Modify: `crates/casad/src/engine.rs`

**Interfaces:**
- Consumes: `crate::mat::{self, Event}`(Task 2)、`crate::rules::{parse_node_id, Trigger::MatterEvent}`(Task 1)
- Produces: `pub fn matter_event_matches(rule: &Rule, config: &Config, event: &mat::Event) -> bool`
- Produces: `pub fn drain_matter_events_once(file: &RuleFile, config: &Config, mat_bin: &str, config_path: Option<&Path>) -> Result<usize, CasaError>`
- Produces: `fn due_matter_event_rules<'a>(file: &'a RuleFile, config: &Config, events: &[mat::Event]) -> Vec<&'a Rule>`(Task 4 の常駐ループも使う)

- [ ] **Step 1: 失敗するテストを書く**

`crates/casad/src/engine.rs` の `mod tests` に追加:

```rust
    fn config_matter() -> Config {
        casa_core::config::parse(
            r#"
version = 1
[devices.study_motion]
protocol = "matter"
node_id = "16"
[devices.desk_tape_light]
protocol = "matter"
node_id = "6"
[devices.outlet2]
protocol = "matter"
node_id = "5678"
endpoint = 2
"#,
        )
        .unwrap()
    }

    fn mat_event(node_id: u64, endpoint: u64, attribute: &str, value: serde_json::Value) -> mat::Event {
        mat::Event {
            node_id,
            endpoint,
            cluster: serde_json::json!("occupancysensing"),
            attribute: serde_json::json!(attribute),
            value,
            priming: false,
        }
    }

    const MATTER_RULE: &str = r#"
version = 1
[[rules]]
name = "人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#;

    #[test]
    fn matter_event_matches_on_node_attribute_value() {
        let file = rules(MATTER_RULE);
        let cfg = config_matter();
        let ev = mat_event(16, 1, "occupancy", serde_json::json!(0));
        assert!(matter_event_matches(&file.rules[0], &cfg, &ev));
    }

    #[test]
    fn matter_event_attribute_is_case_insensitive() {
        let file = rules(MATTER_RULE);
        let cfg = config_matter();
        let ev = mat_event(16, 1, "Occupancy", serde_json::json!(0));
        assert!(matter_event_matches(&file.rules[0], &cfg, &ev));
    }

    #[test]
    fn matter_event_does_not_match_on_mismatch() {
        let file = rules(MATTER_RULE);
        let cfg = config_matter();
        // node_id 違い。
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(6, 1, "occupancy", serde_json::json!(0))
        ));
        // attribute 違い。
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(16, 1, "onoff", serde_json::json!(0))
        ));
        // 値違い（在室 ON）。
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(16, 1, "occupancy", serde_json::json!(1))
        ));
        // 型違い（数値 0 vs 文字列 "0"）。
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(16, 1, "occupancy", serde_json::json!("0"))
        ));
    }

    #[test]
    fn matter_event_priming_never_matches() {
        let file = rules(MATTER_RULE);
        let cfg = config_matter();
        let mut ev = mat_event(16, 1, "occupancy", serde_json::json!(0));
        ev.priming = true;
        // matd 再購読時の現在値再配達で発火してはならない。
        assert!(!matter_event_matches(&file.rules[0], &cfg, &ev));
    }

    #[test]
    fn matter_event_endpoint_filter_applies_only_when_configured() {
        let cfg = config_matter();
        // endpoint = 2 を持つ outlet2 のルール: endpoint 一致のみマッチ。
        let file = rules(
            r#"
version = 1
[[rules]]
name = "outlet2"
when = { device = "outlet2", attribute = "onoff", equals = true }
then = { action = "off", device = "desk_tape_light" }
"#,
        );
        assert!(matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(5678, 2, "onoff", serde_json::json!(true))
        ));
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(5678, 1, "onoff", serde_json::json!(true))
        ));
        // study_motion は endpoint 未指定なのでどの endpoint でもマッチ。
        let file = rules(MATTER_RULE);
        assert!(matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_event(16, 3, "occupancy", serde_json::json!(0))
        ));
    }

    #[test]
    fn matter_numeric_attribute_matches_numeric_rule() {
        // matd は ids テーブルに無い属性を数値のまま流す。ルール側も数値文字列で書けば突合できる。
        let cfg = config_matter();
        let file = rules(
            r#"
version = 1
[[rules]]
name = "数値属性"
when = { device = "study_motion", attribute = "0", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#,
        );
        let mut ev = mat_event(16, 1, "occupancy", serde_json::json!(0));
        ev.attribute = serde_json::json!(0);
        assert!(matter_event_matches(&file.rules[0], &cfg, &ev));
    }

    #[test]
    fn due_matter_event_rules_ignores_echonet_and_time_rules() {
        let cfg = config_matter();
        let file = rules(
            r#"
version = 1
[[rules]]
name = "時刻"
when = { at = "22:00" }
then = { action = "off", device = "desk_tape_light" }
[[rules]]
name = "人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#,
        );
        let ev = mat_event(16, 1, "occupancy", serde_json::json!(0));
        let due = due_matter_event_rules(&file, &cfg, &[ev]);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "人感OFFで消灯");
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p casad --lib engine`
Expected: FAIL(`matter_event_matches` / `due_matter_event_rules` 未定義のコンパイルエラー)

- [ ] **Step 3: 最小実装を書く**

`engine.rs` の `event_matches` 群の下に追加(`use crate::{casa_runner, enl};` を `use crate::{casa_runner, enl, mat};` に変更、`use crate::rules::{parse_node_id, Rule, RuleFile, Trigger};` に変更):

```rust
/// Matter イベントトリガのルールが、与えられた mat listen イベント 1 件に一致するか。
/// priming（matd 再購読時の現在値再配達）は状変ではないので無条件で不一致。
pub fn matter_event_matches(rule: &Rule, config: &Config, event: &mat::Event) -> bool {
    let Trigger::MatterEvent {
        device,
        attribute,
        equals,
    } = &rule.when
    else {
        return false;
    };
    if event.priming {
        return false;
    }
    let (node_id, endpoint) = match config.device(device) {
        Ok(Device::Matter { node_id, endpoint }) => (node_id, endpoint),
        _ => return false,
    };
    if parse_node_id(node_id) != Some(event.node_id) {
        return false;
    }
    if let Some(ep) = endpoint {
        if u64::from(*ep) != event.endpoint {
            return false;
        }
    }
    attribute_matches(&event.attribute, attribute) && event.value == *equals
}

/// イベントの attribute（chip-tool 名 or 未知 ID の数値）とルールの属性名の突合。
/// 名前は case-insensitive、数値はルール側の 10 進表記と比較する。
fn attribute_matches(event_attr: &serde_json::Value, rule_attr: &str) -> bool {
    match event_attr {
        serde_json::Value::String(s) => s.eq_ignore_ascii_case(rule_attr),
        serde_json::Value::Number(n) => rule_attr.trim().parse::<u64>().ok() == n.as_u64(),
        _ => false,
    }
}

/// 1 バッチの mat イベントに一致する Matter イベントトリガのルールを返す
/// （rules.toml 記載順・重複なし）。
fn due_matter_event_rules<'a>(
    file: &'a RuleFile,
    config: &Config,
    events: &[mat::Event],
) -> Vec<&'a Rule> {
    file.rules
        .iter()
        .filter(|r| matches!(r.when, Trigger::MatterEvent { .. }))
        .filter(|r| events.iter().any(|e| matter_event_matches(r, config, e)))
        .collect()
}

/// `mat listen` を 1 回起動し、得たイベントで Matter トリガを発火する。発火件数を返す。
pub fn drain_matter_events_once(
    file: &RuleFile,
    config: &Config,
    mat_bin: &str,
    config_path: Option<&Path>,
) -> Result<usize, CasaError> {
    let events = mat::listen_once(mat_bin)?;
    Ok(fire_all(
        due_matter_event_rules(file, config, &events),
        config_path,
    ))
}
```

`use casa_core::config::{Config, Device};` は既存(確認)。

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p casad && cargo clippy --workspace -- -D warnings`
Expected: 全 PASS(Task 2 の dead_code もここで解消)

- [ ] **Step 5: コミット(Task 2 が未コミットの場合は mat.rs / main.rs も含める)**

```bash
git add crates/casad/src/engine.rs crates/casad/src/mat.rs crates/casad/src/main.rs
git commit -m "feat(casad): mat listen イベントの突合と発火を実装"
```

---

### Task 4: 常駐配線 — mat イベントループ・CLI・統合テスト

**Files:**
- Modify: `crates/casad/src/engine.rs`(`run` 署名変更 + mat ループ)
- Modify: `crates/casad/src/cli.rs`(`--listen-once-mat`)
- Modify: `crates/casad/src/main.rs`(mat bin 解決・分岐)
- Create: `crates/casad/tests/fixtures/mat_listen.sh`
- Modify: `crates/casad/tests/events.rs`(統合テスト追加)

**Interfaces:**
- Consumes: `drain_matter_events_once` / `due_matter_event_rules`(Task 3)、`mat::listen_once`(Task 2)
- Produces: `engine::run(file, config, config_path, enl_bin, mat_bin, opts)`(mat_bin: `&str` を追加)
- Produces: CLI フラグ `casad run <rules> --listen-once-mat`(`--once` / `--listen-once` と排他)

- [ ] **Step 1: fixture を作る**

`crates/casad/tests/fixtures/mat_listen.sh` を新規作成:

```sh
#!/bin/sh
# mat バイナリの代役。`listen` サブコマンド時に固定の occupancy イベント 1 行を出して終了する
# （study_motion = node 16 / endpoint 1 の occupancy が 0=不在 になったイベント）。
# CASAD_MAT_VALUE で value を、CASAD_MAT_PRIMING で priming を上書きできる。
echo "{\"timestamp\":\"2026-07-23T00:00:00+09:00\",\"node_id\":16,\"endpoint\":1,\"cluster\":\"occupancysensing\",\"attribute\":\"occupancy\",\"value\":${CASAD_MAT_VALUE:-0},\"priming\":${CASAD_MAT_PRIMING:-false}}"
exit 0
```

Run: `chmod +x crates/casad/tests/fixtures/mat_listen.sh`

- [ ] **Step 2: 失敗する統合テストを書く**

`crates/casad/tests/events.rs` に追加:

```rust
const MATTER_CONFIG: &str = r#"
version = 1

[devices.study_motion]
protocol = "matter"
node_id = "16"

[devices.desk_tape_light]
protocol = "matter"
node_id = "6"
"#;

const MATTER_RULES: &str = r#"
version = 1
[[rules]]
name = "書斎 人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#;

#[test]
fn listen_once_mat_fires_matter_rule_via_casa() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);
    let rules = write_rules(dir.path(), MATTER_RULES);

    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once-mat",
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
    // occupancy=0 が rule に一致し、casa に `off desk_tape_light` が渡る。
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("off desk_tape_light"), "stdout: {stdout}");
}

#[test]
fn listen_once_mat_does_not_fire_on_priming() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);
    let rules = write_rules(dir.path(), MATTER_RULES);

    // priming（matd 再購読時の現在値再配達）は値が一致しても発火しない。
    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once-mat",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_MAT_BIN", &fixture("mat_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
            ("CASAD_MAT_PRIMING", "true"),
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
fn listen_once_mat_does_not_fire_on_value_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), MATTER_CONFIG);
    let rules = write_rules(dir.path(), MATTER_RULES);

    // occupancy=1（在室）は「OFF で消灯」ルールに一致しない。
    let out = run_casad(
        &[
            "run",
            rules.to_str().unwrap(),
            "--listen-once-mat",
            "--config",
            config.to_str().unwrap(),
        ],
        &[
            ("CASA_MAT_BIN", &fixture("mat_listen.sh")),
            ("CASA_BIN", &fixture("casa_stub.sh")),
            ("CASAD_MAT_VALUE", "1"),
        ],
    );

    assert_eq!(out.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("casa called"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}
```

- [ ] **Step 3: テストが失敗することを確認する**

Run: `cargo test -p casad --test events`
Expected: FAIL(`--listen-once-mat` が未知のフラグ → clap exit 2 で `status.code() == Some(2)`)

- [ ] **Step 4: CLI・main・engine を配線する**

`crates/casad/src/cli.rs` の `Command::Run` にフラグ追加:

```rust
        /// mat listen を 1 回だけ起動し、得たイベントで Matter トリガを評価して終了する
        /// （デバッグ用。イベントが来るまでブロックする）。
        #[arg(long, conflicts_with_all = ["once", "listen_once"])]
        listen_once_mat: bool,
```

`crates/casad/src/main.rs` の `Command::Run` 分岐を更新:

```rust
        Command::Run {
            rules,
            once,
            now,
            listen_once,
            listen_once_mat,
        } => {
            // run も check と同じ検証を通してから起動する（不正ルールで常駐させない）。
            let config = config::load(cli.config.as_deref())?;
            let rule_file = rules::load(&rules)?;
            rule_file.validate(&config)?;
            engine::validate_schedule(&rule_file)?;

            // 子 CLI は casa と同じ規約（CASA_<BIN>_BIN / [binaries] / PATH）で解決する。
            let enl_bin = casa_core::runner::resolve_bin("enl", &config);
            let mat_bin = casa_core::runner::resolve_bin("mat", &config);

            if listen_once {
                // イベント側のデバッグ経路: enl listen を 1 回回して評価し終了する。
                let fired = engine::drain_events_once(
                    &rule_file,
                    &config,
                    &enl_bin,
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
                    cli.config.as_deref(),
                )?;
                tracing::info!(fired, "single matter event drain complete");
                return Ok(0);
            }

            let now = now.map(|s| engine::parse_hm(&s)).transpose()?;
            engine::run(
                &rule_file,
                &config,
                cli.config.as_deref(),
                &enl_bin,
                &mat_bin,
                engine::RunOpts { once, now },
            )
        }
```

`crates/casad/src/engine.rs` の `run` を更新(mat_bin 追加・mat ループ起動):

```rust
/// ルールエンジンを走らせる。`--once` は時刻 1 tick で終了、常駐は時刻スケジューラ
/// （毎分 tick）と イベントリスナ（enl listen / mat listen ループ）を並行に回す。
pub fn run(
    file: &RuleFile,
    config: &Config,
    config_path: Option<&Path>,
    enl_bin: &str,
    mat_bin: &str,
    opts: RunOpts,
) -> Result<i32, CasaError> {
    if opts.once {
        let now = opts.now.unwrap_or_else(|| Local::now().time());
        let fired = tick(file, now, config_path);
        tracing::info!(fired, ?now, "single tick complete");
        return Ok(0);
    }

    tracing::info!("casad resident engine started (time + event)");
    let has_enl_events = file
        .rules
        .iter()
        .any(|r| matches!(r.when, Trigger::Event { .. }));
    let has_matter_events = file
        .rules
        .iter()
        .any(|r| matches!(r.when, Trigger::MatterEvent { .. }));

    // scope で借用を渡し、Arc/clone なしにループ群 + ワーカー群を並行させる。
    // アクション実行はデバイス別ワーカーに非同期投入する（同一デバイス FIFO・
    // 異デバイス並列）。listen / tick ループはアクション完了を待たない。
    std::thread::scope(|s| {
        let dispatcher = Dispatcher::new(s, distinct_devices(file), move |rule: &Rule| {
            run_one(rule, config_path);
        });
        if has_enl_events {
            let d = dispatcher.clone();
            s.spawn(move || event_loop(file, config, enl_bin, &d));
        }
        if has_matter_events {
            let d = dispatcher.clone();
            s.spawn(move || matter_event_loop(file, config, mat_bin, &d));
        }
        time_loop(file, &dispatcher);
    });
    Ok(0) // time_loop は戻らないので到達しない。
}
```

`event_loop` の下に mat 版を追加:

```rust
/// Matter イベントリスナ。`mat listen` を回し続け、一致ルールをワーカーに積んで
/// 即再 listen する。mat 起動失敗・matd 不在（exit 13）はバックオフして再試行。
fn matter_event_loop<'env>(
    file: &'env RuleFile,
    config: &Config,
    mat_bin: &str,
    dispatcher: &Dispatcher<'env>,
) -> ! {
    loop {
        match mat::listen_once(mat_bin) {
            Ok(events) => {
                let queued = dispatcher.dispatch_all(due_matter_event_rules(file, config, &events));
                if queued > 0 {
                    tracing::debug!(queued, "matter event rules queued");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "mat listen failed; backing off");
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}
```

- [ ] **Step 5: 全テスト・lint が通ることを確認する**

Run: `cargo build && cargo test && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check`
Expected: 全 PASS(fmt 差分があれば `cargo fmt --all` を実行してから再確認)

- [ ] **Step 6: コミット**

```bash
git add crates/casad/src/engine.rs crates/casad/src/cli.rs crates/casad/src/main.rs crates/casad/tests/events.rs crates/casad/tests/fixtures/mat_listen.sh
git commit -m "feat(casad): mat listen イベントループと --listen-once-mat を配線"
```

---

### Task 5: ドキュメントとサンプル更新・バージョン bump

**Files:**
- Modify: `README.md`
- Modify: `CLAUDE.md`
- Modify: `examples/rules.toml`
- Modify: `Cargo.toml`(workspace version 1.0.0 → 1.1.0)

**Interfaces:**
- Consumes: Task 1-4 の DSL / CLI 仕様(このタスクはコード変更なし)

- [ ] **Step 1: README.md を更新する**

(1) CLI 前提バージョン表(189-191 行付近)の Matter 行を更新:

```markdown
| Matter | `mat` | 1.0.0 (`read` / `write` / `invoke` / `on` / `off` / `color-temp` / `describe`; casad event triggers additionally need `listen`, which requires a running `matd`) | Supported |
```

(2) casad セクションのルール例(`when = { at = "22:00" }` のブロック)に Matter イベントトリガを追加:

```toml
# Matter event trigger: when study_motion's occupancy becomes 0 (vacant), turn off the desk light
[[rules]]
name = "desk light off when study becomes vacant"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
```

(3) `casad run rules.toml --listen-once` の下にデバッグコマンドを追加:

```bash
# Debug: run mat listen exactly once to evaluate Matter event triggers
casad run rules.toml --listen-once-mat
```

(4) "Event triggers are realized by running enl's `listen` ..." の段落を差し替え:

```markdown
Event triggers are realized by running the child CLI's `listen` in a loop:
`enl listen` (waiting for ECHONET INF notifications) for `epc` triggers, and
`mat listen` (a thin client streaming matd's resident Subscribe; requires a
running `matd`) for `attribute` triggers. Events with `priming: true` (matd's
current-state redelivery on (re)subscribe) never fire rules. Binary resolution
and stderr forwarding follow the same conventions as casa
(`CASA_ENL_BIN` / `CASA_MAT_BIN` / `[binaries]` / `PATH`).
```

- [ ] **Step 2: CLAUDE.md を更新する**

「### Responsibilities the `casad` side takes on」の最初の箇条書きにある
`Time triggers (internal scheduler) / event triggers (run enl listen in a loop and react to INF notifications)` を以下に差し替え:

```markdown
  - Time triggers (internal scheduler) / event triggers (run `enl listen` in a
    loop for ECHONET INF notifications, and `mat listen` in a loop for Matter
    attribute changes via matd's resident Subscribe; `priming: true`
    current-state redeliveries never fire rules)
```

同セクションの `Subscription to ECHONET INF notifications (via enl listen; ...)` の箇条書きの直後に追加:

```markdown
- Subscription to Matter attribute changes (via `mat listen`, a thin client to
  `matd`'s resident Subscribe; requires a running `matd`) — **implemented**
```

- [ ] **Step 3: examples/rules.toml に Matter ルール例を追加する**

末尾に追加(examples/devices.toml の既存 Matter デバイス `living_light` を参照):

```toml
# Matter event trigger: when living_light's onoff attribute becomes true, ... (mat listen via matd).
# equals is compared against the event's JSON value (number / bool / string).
[[rules]]
name = "example matter event trigger"
when = { device = "living_light", attribute = "onoff", equals = true }
then = { action = "on", device = "bedroom_light" }
```

- [ ] **Step 4: サンプルが check を通ることを確認する**

Run: `cargo run -p casad -- check examples/rules.toml --config examples/devices.toml`
Expected: exit 0、stdout の JSON に `"count": 3`

- [ ] **Step 5: workspace version を 1.1.0 に bump する**

`Cargo.toml` の `[workspace.package] version = "1.0.0"` を `"1.1.0"` に変更。

Run: `cargo build`(Cargo.lock 更新)

- [ ] **Step 6: 全体検証とコミット**

Run: `cargo test && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check`
Expected: 全 PASS

```bash
git add README.md CLAUDE.md examples/rules.toml Cargo.toml Cargo.lock
git commit -m "docs: Matter イベントトリガを文書化し 1.1.0 に bump"
```

---

### Task 6: jarvis へのデプロイと実機 E2E

**このタスクはローカルの CI 検証後、ユーザー確認を取ってから実施する**(実機の設定ファイル変更とサービス再起動を含むため)。手順は despliegue skill の規約に従う(cross build → scp)。

**Files(jarvis 側・リポジトリ外):**
- Modify: `jarvis:~/.config/casa/devices.toml`(別リポジトリ管理の実設定)
- Modify: `jarvis:~/.config/casa/rules.toml`

- [ ] **Step 1: despliegue skill を読み、casad を aarch64 で cross build して jarvis へ配布する**

Skill ツールで `despliegue` を起動し、その手順どおりに casad(必要なら casa も)をビルド・配布する。

- [ ] **Step 2: jarvis の devices.toml に study_motion を追加する**

`jarvis:~/.config/casa/devices.toml` に追記(mat alias `study_motion = 16` と対応):

```toml
# 書斎の人感センサー（occupancysensing）。casad の Matter イベントトリガの発火元。
[devices.study_motion]
protocol = "matter"
node_id = "16"
```

devices.toml は別リポジトリ管理なので、リポジトリ化されている場合はそちらの規約でコミットする(状況をユーザーに報告)。

- [ ] **Step 3: jarvis の rules.toml に 2 ルールを追加する**

`jarvis:~/.config/casa/rules.toml` に追記:

```toml
[[rules]]
name = "書斎 人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }

[[rules]]
name = "書斎 人感ONで点灯"
when = { device = "study_motion", attribute = "occupancy", equals = 1 }
then = { action = "on", device = "desk_tape_light" }
```

- [ ] **Step 4: 検証してからサービスを再起動する**

```bash
ssh jarvis 'casad check ~/.config/casa/rules.toml && sudo systemctl restart casad.service && systemctl status casad.service --no-pager | head -5'
```

Expected: check が exit 0(`"ok": true`)、casad.service が active (running)

注意: matd が node 16 の occupancysensing を購読対象にしているか確認する(matd の `only` 設定でクラスタを絞っている場合は occupancysensing が含まれること)。

- [ ] **Step 5: 実機 E2E**

```bash
# イベントが流れてくること自体の確認（人感センサーの前で動く/離れる）
ssh jarvis 'mat listen --node study_motion --count 2 --timeout-ms 120000'
# casad のログで発火を確認
ssh jarvis 'journalctl -u casad.service --since "5 min ago" --no-pager | tail -20'
```

Expected: 在室 → `desk_tape_light` 点灯、退室(センサーの保持時間経過)→ 消灯。journalctl に `firing rule` が残る。

---

## Self-Review 済み事項

- spec の全要件(DSL・mat.rs・突合・priming スキップ・ループ・validate・テスト・デプロイ)に対応する Task がある
- 型整合: `parse_node_id` は Task 1 定義 → Task 3 で使用。`mat::Event` は Task 2 定義 → Task 3/4 で使用。`engine::run` の署名変更は Task 4 で main.rs と同時に行うため各 Task 完了時点でビルドが通る
- Task 2 の dead_code 警告リスク(bin クレートの未使用 pub)は Task 3 との合流手順を明記済み
