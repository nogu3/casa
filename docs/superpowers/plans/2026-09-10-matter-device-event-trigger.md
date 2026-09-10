# Matter デバイスイベントトリガ Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** casad の rules.toml に `when = { device, event, data? }` 構文を追加し、`mat listen` の Matter デバイスイベント行（`event` キー）でルールを発火できるようにする。

**Architecture:** `mat.rs` が属性行とイベント行を `Incoming` enum に分類して 1 本のコールバックへ流す。`rules.rs` の untagged `Trigger` に `MatterEvent { device, event, data }` を足し、既存の属性トリガは `MatterAttribute` に改名する。`engine.rs` は `Incoming` ごとにルール突合を行い、常駐ストリームでは新規 `event_dedupe.rs` の `EventWatermark` で `event_number` の重複を落としてから発火する。

**Tech Stack:** Rust 1.98 (edition 2021), serde / serde_json / toml, tracing。テストは `cargo test -p casad`。

**Spec:** `docs/superpowers/specs/2026-09-10-matter-device-event-trigger-design.md`

## Global Constraints

- casa/casad の設計原則（CLAUDE.md）: casad(bin) は状態を in-memory のみ保持。ファイルキャッシュ・DB は禁止。
- ルールの TOML 構文は既存ルールに対して**後方互換**（`attribute` トリガの書き方は不変）。
- `priming: true` の行は属性・イベントとも**絶対に発火しない**。
- 突合の値比較は JSON 値として型込み（`2` と `2.0` は不一致）。
- コミットは各タスク末尾で行い、`cargo fmt --all` → `cargo clippy --workspace -- -D warnings` → `cargo test --workspace` を通してから。
- コミットメッセージは日本語、既存の形式 `feat(casad): …` / `refactor(casad): …` / `docs: …` に合わせ、末尾に次の 2 行を付ける:
  ```
  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01YVQ9U3CMVfuo12TkcxF73f
  ```
- 作業ディレクトリは worktree `/home/noguk/ghq/github.com/nogu3/casa-wt/event-trigger`（ブランチ `feat/event-trigger`）。push はしない。
- リポジトリは公開。例には RFC 5737 のダミー IP / ダミー node_id のみ。

## ファイル構成

| ファイル | 責務 | 変更 |
|---|---|---|
| `crates/casad/src/rules.rs` | rules.toml の型・パース・検証 | `Trigger::MatterAttribute`（改名）、`Trigger::MatterEvent`（新規）、空 `data` の拒否 |
| `crates/casad/src/mat.rs` | `mat listen` の起動・行パース | `AttributeChange`（改名）、`MatterEvent` に `event_number` / `data`、`Incoming` enum、コールバックと `listen_once` の型変更 |
| `crates/casad/src/engine.rs` | ルール評価・発火・常駐ループ | `matter_attribute_matches`（改名）、`matter_event_matches`（新規）、`due_matter_rules`（`Incoming` 混在バッチ）、ループのデデュープ配線 |
| `crates/casad/src/event_dedupe.rs` | **新規**: `EventWatermark` | event_number 単調性デデュープ |
| `crates/casad/src/dispatch.rs` | ワーカー送信ポリシー | `grace_applies` の match に新 variant を追加 |
| `crates/casad/src/state_cache.rs` | OnOff 最終値キャッシュ | 型名の追従のみ |
| `crates/casad/src/main.rs` | エントリ | `mod event_dedupe;` 追加 |
| `README.md` / `examples/rules.toml` / `CLAUDE.md` / `Cargo.toml` | ドキュメントとバージョン | 節の改名・新設、例の追加、1.9.0 |

---

### Task 1: 属性トリガの改名（純リファクタ）

既存の「Matter event trigger」は実際には属性トリガなので、本物のイベントトリガを足す前に名前を空ける。TOML 構文・挙動は不変。

**Files:**
- Modify: `crates/casad/src/rules.rs`（`Trigger` の variant 名・doc、validate、tests）
- Modify: `crates/casad/src/mat.rs`（`Event` → `AttributeChange`）
- Modify: `crates/casad/src/engine.rs`（関数名・型名・tests）
- Modify: `crates/casad/src/dispatch.rs:135`
- Modify: `crates/casad/src/state_cache.rs`（`mat::Event` → `mat::AttributeChange`）

**Interfaces:**
- Produces: `rules::Trigger::MatterAttribute { device: String, attribute: String, equals: serde_json::Value }`（旧 `MatterEvent`）
- Produces: `mat::AttributeChange`（旧 `mat::Event`、フィールド不変）
- Produces: `engine::matter_attribute_matches(rule: &Rule, config: &Config, event: &mat::AttributeChange) -> bool`（旧 `matter_event_matches`）
- Produces: `engine::due_matter_rules(file: &RuleFile, config: &Config, events: &[mat::AttributeChange], now: NaiveTime) -> Vec<&Rule>`（旧 `due_matter_event_rules`。Task 2 で引数型が `&[mat::Incoming]` に変わる）

- [ ] **Step 1: ベースラインのテストが通ることを確認**

Run: `cargo test -p casad 2>&1 | tail -5`
Expected: `test result: ok.`

- [ ] **Step 2: 機械的に改名する**

```bash
cd /home/noguk/ghq/github.com/nogu3/casa-wt/event-trigger
sed -i 's/Trigger::MatterEvent\b/Trigger::MatterAttribute/g' crates/casad/src/rules.rs crates/casad/src/engine.rs crates/casad/src/dispatch.rs
sed -i 's/^    MatterEvent {$/    MatterAttribute {/' crates/casad/src/rules.rs
sed -i 's/\bmat::Event\b/mat::AttributeChange/g' crates/casad/src/engine.rs crates/casad/src/state_cache.rs
sed -i 's/\bEvent\b/AttributeChange/g' crates/casad/src/mat.rs
sed -i 's/matter_event_matches/matter_attribute_matches/g; s/due_matter_event_rules/due_matter_rules/g' crates/casad/src/engine.rs
sed -i 's/fn matter_event_attribute_is_case_insensitive/fn matter_attribute_is_case_insensitive/; s/fn matter_event_does_not_match_on_mismatch/fn matter_attribute_does_not_match_on_mismatch/; s/fn matter_event_priming_never_matches/fn matter_attribute_priming_never_matches/; s/fn matter_event_endpoint_filter_applies_only_when_configured/fn matter_attribute_endpoint_filter_applies_only_when_configured/' crates/casad/src/engine.rs
sed -i 's/fn parses_matter_event_rule/fn parses_matter_attribute_rule/; s/fn validate_accepts_matter_event_rule/fn validate_accepts_matter_attribute_rule/' crates/casad/src/rules.rs
grep -n 'MatterAttribute {$' crates/casad/src/rules.rs   # 1 件（enum の variant 定義）だけのはず
grep -n '\bEvent\b' crates/casad/src/mat.rs                 # 0 件のはず
```

`sed 's/\bEvent\b/…/'` は `MatterEvent` には当たらない（`r` と `E` の間に語境界が無い）。`mat.rs` の `enum Line { Attribute(AttributeChange), MatterEvent(MatterEvent), … }` はこの時点ではそのまま（Task 2 で置き換える）。

- [ ] **Step 3: doc コメントを意味に合わせて直す**

`crates/casad/src/rules.rs` の variant doc（元は「Matter イベント: あるデバイスの属性が指定値になったとき」）を次に差し替える:

```rust
    /// Matter 属性: あるデバイスの属性が指定値になったとき（mat listen の属性行経由）。
    /// 例: `when = { device = "study_motion", attribute = "occupancy", equals = 0 }`
    /// equals は matd 属性行の JSON `value` と等値比較する（数値 / bool / 文字列）。
    MatterAttribute {
```

`crates/casad/src/mat.rs` の `AttributeChange` の doc を次に差し替える（元「`mat listen` の 1 イベント行。casad が使うフィールドのみ拾う。」）:

```rust
/// `mat listen` の属性変化行（`attribute` キーあり）。casad が使うフィールドのみ拾う。
#[derive(Debug, Deserialize)]
pub struct AttributeChange {
```

`crates/casad/src/engine.rs` の `matter_attribute_matches` の doc（元「Matter イベントトリガのルールが、与えられた mat listen イベント 1 件に一致するか。」）を「Matter 属性トリガのルールが、与えられた mat listen の属性行 1 件に一致するか。」に、`due_matter_rules` の doc を「1 バッチの mat listen 行に一致する Matter 由来トリガのルールを返す（rules.toml 記載順・重複なし）。」に直す。`check_matter_device` のエラー文言 `"matter event trigger requires a matter device"` はイベントトリガでも共用するのでそのまま。

- [ ] **Step 4: fmt / clippy / test が通ることを確認**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|error"`
Expected: すべて `test result: ok.`、error なし。

- [ ] **Step 5: コミット**

```bash
git add crates/casad/src/rules.rs crates/casad/src/mat.rs crates/casad/src/engine.rs crates/casad/src/dispatch.rs crates/casad/src/state_cache.rs
git commit -m "refactor(casad): Matter 属性トリガを MatterAttribute に改名し event の名前を空ける

TOML 構文・挙動は不変。mat::Event → mat::AttributeChange、
engine の matter_event_matches → matter_attribute_matches、
due_matter_event_rules → due_matter_rules。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01YVQ9U3CMVfuo12TkcxF73f"
```

---

### Task 2: `mat.rs` — イベント行を `Incoming` として流す

**Files:**
- Modify: `crates/casad/src/mat.rs`
- Modify: `crates/casad/src/engine.rs`（`drain_matter_events_once`、`matter_event_loop`、`due_matter_rules` の型）

**Interfaces:**
- Consumes: Task 1 の `mat::AttributeChange`、`engine::due_matter_rules`
- Produces:
  ```rust
  pub struct MatterEvent {
      pub node_id: u64,
      pub endpoint: u64,
      pub cluster: serde_json::Value,
      pub event: serde_json::Value,
      pub event_number: Option<u64>,                                   // 欠落 = None
      pub data: Option<serde_json::Map<String, serde_json::Value>>,    // キー無し = None
      pub priming: bool,
  }
  pub enum Incoming { Attribute(AttributeChange), Event(MatterEvent) }
  pub fn listen_stream(bin: &str, on_line: impl FnMut(Incoming)) -> Result<(), CasaError>
  pub fn listen_once(bin: &str) -> Result<Vec<Incoming>, CasaError>
  ```
- Produces: `engine::due_matter_rules(file, config, lines: &[mat::Incoming], now) -> Vec<&Rule>`（この Task では `Incoming::Event` は常に不一致）

- [ ] **Step 1: 失敗するテストを書く**

`crates/casad/src/mat.rs` の `mod tests` 内で、既存の `read_events_ignores_matter_event_lines_silently` を**削除**し、代わりに以下を追加する。また既存テストのクロージャを `Incoming` に合わせて書き換える。

```rust
    /// テスト用: Incoming から属性行だけ取り出す。
    fn attr(line: Incoming) -> Option<AttributeChange> {
        match line {
            Incoming::Attribute(ev) => Some(ev),
            Incoming::Event(_) => None,
        }
    }

    #[test]
    fn parses_single_event_line() {
        let line = br#"{"timestamp":"2026-07-23T00:00:00+09:00","node_id":16,"endpoint":1,"cluster":"occupancysensing","attribute":"occupancy","value":1,"priming":false}"#;
        let lines = parse_lines(line).unwrap();
        assert_eq!(lines.len(), 1);
        let Incoming::Attribute(ev) = &lines[0] else {
            panic!("expected attribute line: {:?}", lines[0])
        };
        assert_eq!(ev.node_id, 16);
        assert_eq!(ev.endpoint, 1);
        assert_eq!(ev.attribute, serde_json::json!("occupancy"));
        assert_eq!(ev.value, serde_json::json!(1));
        assert!(!ev.priming);
    }

    #[test]
    fn parses_multiple_lines_and_skips_blank() {
        let lines = b"{\"node_id\":16,\"endpoint\":1,\"cluster\":1030,\"attribute\":0,\"value\":0,\"priming\":true}\n\n{\"node_id\":6,\"endpoint\":1,\"cluster\":\"onoff\",\"attribute\":\"onoff\",\"value\":true}\n";
        let events: Vec<AttributeChange> = parse_lines(lines)
            .unwrap()
            .into_iter()
            .filter_map(attr)
            .collect();
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

    /// ストリーム読みは 1 行ごとにコールバックへ渡す（到着順）。空行は無視。
    #[test]
    fn read_events_delivers_each_line_in_order() {
        let lines = b"{\"node_id\":16,\"endpoint\":1,\"cluster\":\"occupancysensing\",\"attribute\":\"occupancy\",\"value\":0,\"priming\":false}\n\n{\"node_id\":16,\"endpoint\":1,\"cluster\":\"occupancysensing\",\"attribute\":\"occupancy\",\"value\":1,\"priming\":false}\n";
        let mut seen = Vec::new();
        read_events(&lines[..], |l| seen.extend(attr(l).map(|ev| ev.value)));
        assert_eq!(seen, vec![serde_json::json!(0), serde_json::json!(1)]);
    }

    /// パース不能行（matd の lag エラー行等）は skip し、ストリームは続行する。
    /// one-shot 時代の「1 行不正で全滅」をやめるのが狙い。
    #[test]
    fn read_events_skips_invalid_line_and_continues() {
        let lines = b"{\"node_id\":6,\"endpoint\":1,\"cluster\":\"onoff\",\"attribute\":\"onoff\",\"value\":true}\n{\"error\":{\"kind\":\"lagged\",\"detail\":\"...\"}}\n{\"node_id\":7,\"endpoint\":1,\"cluster\":\"onoff\",\"attribute\":\"onoff\",\"value\":false}\n";
        let mut seen = Vec::new();
        read_events(&lines[..], |l| seen.extend(attr(l).map(|ev| ev.node_id)));
        assert_eq!(seen, vec![6, 7]);
    }

    /// recovered フィールドを拾う（欠落は false 既定）。発火判定には使わず
    /// debug ログで盲目期間回復を追えるようにする。
    #[test]
    fn read_events_parses_recovered_field() {
        let lines = b"{\"node_id\":16,\"endpoint\":1,\"cluster\":\"occupancysensing\",\"attribute\":\"occupancy\",\"value\":1,\"priming\":false,\"recovered\":true}\n{\"node_id\":16,\"endpoint\":1,\"cluster\":\"occupancysensing\",\"attribute\":\"occupancy\",\"value\":0,\"priming\":false}\n";
        let mut seen = Vec::new();
        read_events(&lines[..], |l| seen.extend(attr(l).map(|ev| ev.recovered)));
        assert_eq!(seen, vec![true, false]);
    }

    /// 属性行とイベント行（mat 1.36.0+、`event` キーあり）を到着順のまま `Incoming` に
    /// 分類して渡す。イベント行は `event_number` / `data` / `priming` を拾う。
    #[test]
    fn read_events_classifies_attribute_and_event_lines_in_order() {
        let lines = b"{\"node_id\":19,\"endpoint\":1,\"cluster\":\"booleanstate\",\"attribute\":\"state-value\",\"value\":false,\"priming\":false}\n{\"timestamp\":\"2026-09-07T00:37:33+09:00\",\"node_id\":19,\"endpoint\":1,\"cluster\":\"booleanstate\",\"event\":\"state-change\",\"event_number\":590151,\"priority\":\"info\",\"data\":{\"state-value\":false},\"device_time\":{\"system_ms\":577001426},\"priming\":true}\n{\"node_id\":25,\"endpoint\":2,\"cluster\":\"switch\",\"event\":\"multi-press-complete\",\"event_number\":1725600000130,\"priority\":\"info\",\"data\":{\"previous-position\":1,\"total-number-of-presses-counted\":2},\"priming\":false}\n";
        let mut seen = Vec::new();
        read_events(&lines[..], |l| seen.push(l));
        assert_eq!(seen.len(), 3);
        assert!(matches!(&seen[0], Incoming::Attribute(ev) if ev.node_id == 19));
        let Incoming::Event(door) = &seen[1] else {
            panic!("expected event line: {:?}", seen[1])
        };
        assert_eq!(door.node_id, 19);
        assert_eq!(door.endpoint, 1);
        assert_eq!(door.cluster, serde_json::json!("booleanstate"));
        assert_eq!(door.event, serde_json::json!("state-change"));
        assert_eq!(door.event_number, Some(590151));
        assert_eq!(
            door.data.as_ref().unwrap().get("state-value"),
            Some(&serde_json::json!(false))
        );
        assert!(door.priming);
        let Incoming::Event(press) = &seen[2] else {
            panic!("expected event line: {:?}", seen[2])
        };
        assert_eq!(press.event_number, Some(1725600000130));
        assert_eq!(
            press.data.as_ref().unwrap().get("total-number-of-presses-counted"),
            Some(&serde_json::json!(2))
        );
        assert!(!press.priming);
    }

    /// data キー無し（イベントがフィールドを持たない）と event_number 欠落は None。
    /// 未知 ID の数値 cluster / event もそのまま拾う。
    #[test]
    fn event_line_without_data_or_number_parses_to_none() {
        let start_up = classify_line(
            r#"{"node_id":19,"endpoint":0,"cluster":"basicinformation","event":"start-up","event_number":1,"priority":"critical","priming":true}"#,
        )
        .unwrap();
        let Incoming::Event(ev) = start_up else {
            panic!("expected event line")
        };
        assert!(ev.data.is_none());
        assert_eq!(ev.event_number, Some(1));

        let numeric = classify_line(r#"{"node_id":19,"endpoint":1,"cluster":59,"event":1}"#).unwrap();
        let Incoming::Event(ev) = numeric else {
            panic!("expected event line")
        };
        assert_eq!(ev.cluster, serde_json::json!(59));
        assert_eq!(ev.event, serde_json::json!(1));
        assert!(ev.event_number.is_none());
        assert!(!ev.priming);

        // どちらのキーも無い・必須欠落は Err。
        assert!(classify_line(r#"{"error":{"kind":"lagged","detail":"..."}}"#).is_err());
        assert!(classify_line(r#"{"node_id":19,"event":"start-up"}"#).is_err());
    }

    /// listen_once 用の parse_lines も混在バッチをそのまま返す。
    #[test]
    fn parse_lines_keeps_mixed_batch() {
        let lines = b"{\"node_id\":6,\"endpoint\":1,\"cluster\":\"onoff\",\"attribute\":\"on-off\",\"value\":true}\n{\"node_id\":25,\"endpoint\":2,\"cluster\":\"switch\",\"event\":\"initial-press\",\"event_number\":3,\"data\":{\"new-position\":1}}\n";
        let lines = parse_lines(lines).unwrap();
        assert!(matches!(lines[0], Incoming::Attribute(_)));
        assert!(matches!(lines[1], Incoming::Event(_)));
    }
```

- [ ] **Step 2: テストがコンパイルエラーで失敗することを確認**

Run: `cargo test -p casad mat:: 2>&1 | grep -E "^error|cannot find" | head -5`
Expected: `Incoming` / `classify_line` の戻り値型に関するコンパイルエラー。

- [ ] **Step 3: `mat.rs` を実装する**

`MatterEvent` 構造体と `Line` enum・`classify_line` を次で置き換える:

```rust
/// `mat listen` の Matter デバイスイベント行（mat 1.36.0+、`event` キーあり・`attribute` 無し）。
/// wildcard 購読では起動系イベント（start-up / boot-reason / access-control-entry-changed）
/// の priming が数十行まとめて来る。発火判定（priming 除外・突合）は engine、重複配達の
/// デデュープは event_dedupe が担う。
#[derive(Debug, Deserialize)]
pub struct MatterEvent {
    pub node_id: u64,
    pub endpoint: u64,
    /// chip-tool 名（例 "switch"）または未知 ID の数値。
    pub cluster: serde_json::Value,
    /// イベント名（例 "initial-press"）または未知 ID の数値。
    pub event: serde_json::Value,
    /// Matter の EventNumber。デデュープに使う。欠落は None（デデュープ対象外）。
    #[serde(default)]
    pub event_number: Option<u64>,
    /// イベントフィールド（kebab 名キー）。イベントがデータを持たなければキー自体が無い。
    #[serde(default)]
    pub data: Option<serde_json::Map<String, serde_json::Value>>,
    /// matd（再）購読時の再配達。状変ではないので発火してはならない。
    #[serde(default)]
    pub priming: bool,
}

/// `mat listen` の 1 行（属性行 / イベント行）。engine はこれを受けて突合する。
#[derive(Debug)]
pub enum Incoming {
    Attribute(AttributeChange),
    Event(MatterEvent),
}

/// 属性行とイベント行を `attribute` / `event` キーの有無で分ける。両方無い・
/// 必須フィールド欠落は Err（matd の lag エラー行 `{"error":...}` 等）。
fn classify_line(line: &str) -> Result<Incoming, serde_json::Error> {
    let has_key = |key: &str| {
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(line)
            .map(|m| m.contains_key(key))
            .unwrap_or(false)
    };
    if has_key("event") && !has_key("attribute") {
        return serde_json::from_str::<MatterEvent>(line).map(Incoming::Event);
    }
    serde_json::from_str::<AttributeChange>(line).map(Incoming::Attribute)
}

/// 受信した 1 行を debug で残す（ルールに一致しない行も追えるよう全件）。
fn log_received(line: &Incoming) {
    match line {
        Incoming::Attribute(ev) => tracing::debug!(
            node_id = ev.node_id,
            endpoint = ev.endpoint,
            cluster = %ev.cluster,
            attribute = %ev.attribute,
            value = %ev.value,
            priming = ev.priming,
            recovered = ev.recovered,
            "event received"
        ),
        Incoming::Event(ev) => tracing::debug!(
            node_id = ev.node_id,
            endpoint = ev.endpoint,
            cluster = %ev.cluster,
            event = %ev.event,
            event_number = ?ev.event_number,
            data = ?ev.data,
            priming = ev.priming,
            "matter event received"
        ),
    }
}
```

`listen_stream` のシグネチャを `pub fn listen_stream(bin: &str, on_line: impl FnMut(Incoming)) -> Result<(), CasaError>` にし、内部の `read_events(stdout, on_event)` 呼び出しの引数名も合わせる。`read_events` を次で置き換える:

```rust
/// NDJSON ストリームを 1 行ずつ読み、属性行 / イベント行を `on_line` へ渡す。
/// 空行は無視。パース不能行（matd の lag エラー行 `{"error":...}` 等）は warn を
/// 残して skip し、ストリームは続行する。EOF / 読み取りエラーで戻る。
fn read_events<R: BufRead>(reader: R, mut on_line: impl FnMut(Incoming)) {
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "mat listen stdout read failed");
                return;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        match classify_line(&line) {
            Ok(incoming) => {
                log_received(&incoming);
                on_line(incoming);
            }
            Err(e) => {
                tracing::warn!(error = %e, line, "mat listen stdout line is not an event; skipping");
            }
        }
    }
}
```

`listen_once` の戻り値を `Result<Vec<Incoming>, CasaError>` にし、末尾の debug ループを `for line in &lines { log_received(line); }` に置き換える（変数名 `events` → `lines`）。`parse_lines` を次で置き換える:

```rust
/// stdout（1 行 1 JSON）を Incoming 列にパースする。空行は無視。
fn parse_lines(stdout: &[u8]) -> Result<Vec<Incoming>, CasaError> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            classify_line(line).map_err(|e| {
                CasaError::new(
                    ErrorKind::ChildInvalidOutput,
                    format!("mat listen stdout line is not valid JSON: {e}"),
                )
            })
        })
        .collect()
}
```

モジュール doc 冒頭の「casad はまだイベントをトリガに使わない」系の記述があれば削除する。

- [ ] **Step 4: `engine.rs` を新しい型に追従させる（イベント行はまだ不一致）**

`due_matter_rules` を次で置き換える:

```rust
/// 1 バッチの mat listen 行（属性行・イベント行混在）に一致する Matter 由来トリガの
/// ルールを返す（rules.toml 記載順・重複なし）。
fn due_matter_rules<'a>(
    file: &'a RuleFile,
    config: &Config,
    lines: &[mat::Incoming],
    now: NaiveTime,
) -> Vec<&'a Rule> {
    file.rules
        .iter()
        .filter(|r| {
            lines.iter().any(|l| match l {
                mat::Incoming::Attribute(ev) => matter_attribute_matches(r, config, ev),
                mat::Incoming::Event(_) => false,
            })
        })
        .filter(|r| active_or_log(r, now))
        .collect()
}
```

`drain_matter_events_once` 内の `let events = mat::listen_once(mat_bin)?;` → `let lines = mat::listen_once(mat_bin)?;` とし、`due_matter_rules(file, config, &lines, now_or(now))` を渡す。

`matter_event_loop` のクロージャを次に置き換える:

```rust
        let result = mat::listen_stream(mat_bin, |incoming| {
            if let mat::Incoming::Attribute(ev) = &incoming {
                // off no-op スキップ用の最終値キャッシュを先に更新する（priming 含む —
                // 現在値の再配達はキャッシュには有効な知識）。
                cache.record(ev);
            }
            // 窓判定はイベント受信時刻で行う（ストリームは何時間も継続しうる）。
            let now = Local::now().time();
            let queued = dispatcher.dispatch_all(due_matter_rules(
                file,
                config,
                std::slice::from_ref(&incoming),
                now,
            ));
            if queued > 0 {
                tracing::debug!(queued, "matter event rule actions queued");
            }
        });
```

engine の既存テストで `due_matter_rules(&file, &cfg, &[ev], …)` と呼んでいる箇所（`due_matter_rules_ignores_echonet_and_time_rules`、`due_matter_rules_respects_active_window`）は `&[mat::Incoming::Attribute(ev)]` / `&[mat::Incoming::Attribute(mat_event(16, 1, "occupancy", serde_json::json!(0)))]` に書き換える。

- [ ] **Step 5: テスト・clippy が通ることを確認**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|error"`
Expected: すべて `test result: ok.`。

- [ ] **Step 6: コミット**

```bash
git add crates/casad/src/mat.rs crates/casad/src/engine.rs
git commit -m "feat(casad): mat listen のイベント行を Incoming として engine へ流す

MatterEvent に event_number / data を追加し、属性行と一緒に 1 本の
コールバックへ渡す。ルール構文はまだ無いのでイベント行は不一致。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01YVQ9U3CMVfuo12TkcxF73f"
```

---

### Task 3: `rules.rs` — `Trigger::MatterEvent` 構文

**Files:**
- Modify: `crates/casad/src/rules.rs`（`Trigger`、`parse`、`validate`、tests）
- Modify: `crates/casad/src/engine.rs:175-180`（`due_time_rules` の match）、`has_matter_events`
- Modify: `crates/casad/src/dispatch.rs:135`（`grace_applies`）

**Interfaces:**
- Produces:
  ```rust
  Trigger::MatterEvent {
      device: String,
      event: String,
      data: Option<serde_json::Map<String, serde_json::Value>>,   // 省略 = None、空テーブルは parse エラー
  }
  ```

- [ ] **Step 1: 失敗するテストを書く**

`crates/casad/src/rules.rs` の `mod tests` に追加（`config_with_matter` の後ろ）:

```rust
    #[test]
    fn parses_matter_device_event_rule_with_data() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "2回押しで消灯"
when = { device = "wall_switch", event = "multi-press-complete", data = { "total-number-of-presses-counted" = 2 } }
then = { action = "off", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        match &file.rules[0].when {
            Trigger::MatterEvent {
                device,
                event,
                data,
            } => {
                assert_eq!(device, "wall_switch");
                assert_eq!(event, "multi-press-complete");
                let data = data.as_ref().expect("data が None");
                assert_eq!(
                    data.get("total-number-of-presses-counted"),
                    Some(&serde_json::json!(2))
                );
            }
            other => panic!("unexpected trigger: {other:?}"),
        }
    }

    #[test]
    fn parses_matter_device_event_rule_without_data() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "起動検知"
when = { device = "study_motion", event = "start-up" }
then = { action = "on", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        match &file.rules[0].when {
            Trigger::MatterEvent { event, data, .. } => {
                assert_eq!(event, "start-up");
                assert!(data.is_none());
            }
            other => panic!("unexpected trigger: {other:?}"),
        }
        // JSON 化しても data は省略される（casad check の出力）。
        let v = serde_json::to_value(&file.rules).unwrap();
        assert!(v[0]["when"].get("data").is_none());
    }

    #[test]
    fn matter_device_event_rule_rejects_empty_data() {
        // 空の data は「省略」と区別がつかず書き間違いで無条件発火するので弾く。
        let err = parse(
            r#"
version = 1
[[rules]]
name = "空data"
when = { device = "study_motion", event = "state-change", data = {} }
then = { action = "on", device = "desk_tape_light" }
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("空data"), "{}", err.detail);
        assert!(err.detail.contains("data"), "{}", err.detail);
    }

    #[test]
    fn attribute_rule_still_parses_as_matter_attribute() {
        // attribute キーは従来どおり MatterAttribute に落ちる（MatterEvent に吸われない）。
        let file = parse(
            r#"
version = 1
[[rules]]
name = "属性"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        assert!(matches!(file.rules[0].when, Trigger::MatterAttribute { .. }));
    }

    #[test]
    fn validate_accepts_matter_device_event_rule() {
        let file = parse(
            r#"
version = 1
[[rules]]
name = "起動検知"
when = { device = "study_motion", event = "start-up" }
then = { action = "on", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        file.validate(&config_with_matter()).unwrap();
    }

    #[test]
    fn validate_rejects_matter_device_event_rule_on_non_matter_or_group() {
        // ECHONET デバイスは不可。
        let file = parse(
            r#"
version = 1
[[rules]]
name = "echonet"
when = { device = "washstand_light", event = "start-up" }
then = { action = "on", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        let err = file.validate(&config_with_matter()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ProtocolUnsupported);
        assert!(err.detail.contains("echonet"));

        // groupcast デバイス（node_id 無し）も不可。
        let cfg = casa_core::config::parse(
            r#"
version = 1
[devices.hall_lights]
protocol = "matter"
group = "hall_group"
[devices.desk_tape_light]
protocol = "matter"
node_id = "6"
"#,
        )
        .unwrap();
        let file = parse(
            r#"
version = 1
[[rules]]
name = "group"
when = { device = "hall_lights", event = "start-up" }
then = { action = "on", device = "desk_tape_light" }
"#,
        )
        .unwrap();
        let err = file.validate(&cfg).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConfigParse);
        assert!(err.detail.contains("group"));
    }
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p casad rules:: 2>&1 | grep -E "^error|no variant" | head -5`
Expected: `Trigger::MatterEvent` が無い旨のコンパイルエラー。

- [ ] **Step 3: `Trigger` に variant を追加する**

`crates/casad/src/rules.rs` の `Trigger` enum、`MatterAttribute` の直後（`Time` の前）に追加:

```rust
    /// Matter デバイスイベント: あるデバイスが指定イベントを発したとき（mat listen の
    /// event 行、mat 1.36.0+）。`data` は省略可。書いたキーだけをイベントの `data` と
    /// JSON 値として型込みで等値比較し、書いていないキーは無視する（部分一致）。
    /// 例: `when = { device = "wall_switch", event = "multi-press-complete", data = { "total-number-of-presses-counted" = 2 } }`
    MatterEvent {
        device: String,
        event: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Map<String, serde_json::Value>>,
    },
```

untagged の判別キーは `epc` / `attribute` / `event` / `at` で互いに素なので、順序はエラー文言にしか影響しない。

`parse` 関数のルールごとのループ（`then is empty` の検査）に追加:

```rust
        if let Trigger::MatterEvent {
            data: Some(data), ..
        } = &rule.when
        {
            if data.is_empty() {
                return Err(CasaError::new(
                    ErrorKind::ConfigParse,
                    format!(
                        "rule \"{}\": when.data is empty; omit data to match on the event name alone",
                        rule.name
                    ),
                ));
            }
        }
```

`RuleFile::validate` の match に追加:

```rust
                Trigger::MatterEvent { device, .. } => {
                    check_matter_device(config, &rule.name, device)?;
                }
```

- [ ] **Step 4: engine / dispatch の網羅 match を追従させる**

`crates/casad/src/engine.rs` の `due_time_rules`:

```rust
            Trigger::Event { .. } => false,
            Trigger::MatterAttribute { .. } => false,
            Trigger::MatterEvent { .. } => false,
```

同じく `run` 内の `has_matter_events`:

```rust
    let has_matter_events = file.rules.iter().any(|r| {
        matches!(
            r.when,
            Trigger::MatterAttribute { .. } | Trigger::MatterEvent { .. }
        )
    });
```

`crates/casad/src/dispatch.rs` の `grace_applies`（イベント由来の off に猶予を適用する判定。デバイスイベントも連射源なので含める）:

```rust
        && matches!(
            job.rule.when,
            Trigger::Event { .. } | Trigger::MatterAttribute { .. } | Trigger::MatterEvent { .. }
        )
```

- [ ] **Step 5: テスト・clippy が通ることを確認**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|error"`
Expected: すべて `test result: ok.`。

- [ ] **Step 6: コミット**

```bash
git add crates/casad/src/rules.rs crates/casad/src/engine.rs crates/casad/src/dispatch.rs
git commit -m "feat(casad): rules.toml に Matter デバイスイベントトリガ構文 when = { device, event, data? } を追加

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01YVQ9U3CMVfuo12TkcxF73f"
```

---

### Task 4: `engine.rs` — イベント行の突合と発火

**Files:**
- Modify: `crates/casad/src/engine.rs`

**Interfaces:**
- Consumes: Task 2 の `mat::Incoming` / `mat::MatterEvent`、Task 3 の `Trigger::MatterEvent`
- Produces:
  ```rust
  pub fn matter_event_matches(rule: &Rule, config: &Config, event: &mat::MatterEvent) -> bool
  fn source_matches(config: &Config, device: &str, node_id: u64, endpoint: u64) -> bool
  fn name_matches(actual: &serde_json::Value, rule_name: &str) -> bool   // 旧 attribute_matches
  ```
  `due_matter_rules` が `Incoming::Event` でも一致するようになる。

- [ ] **Step 1: 失敗するテストを書く**

`crates/casad/src/engine.rs` の `mod tests` で、`config_matter()` の TOML に次の 2 デバイスを追加する（`outlet2` の後ろ）:

```toml
[devices.wall_switch]
protocol = "matter"
node_id = "25"
endpoint = 2
[devices.door_sensor]
protocol = "matter"
node_id = "19"
```

同じ `mod tests` にヘルパとテストを追加する（`due_matter_rules_ignores_echonet_and_time_rules` の後ろ）:

```rust
    fn mat_device_event(
        node_id: u64,
        endpoint: u64,
        cluster: &str,
        event: &str,
        data: Option<serde_json::Value>,
    ) -> mat::MatterEvent {
        mat::MatterEvent {
            node_id,
            endpoint,
            cluster: serde_json::json!(cluster),
            event: serde_json::json!(event),
            event_number: Some(1),
            data: data.map(|d| d.as_object().expect("data はオブジェクト").clone()),
            priming: false,
        }
    }

    const SWITCH_RULE: &str = r#"
version = 1
[[rules]]
name = "2回押しで消灯"
when = { device = "wall_switch", event = "multi-press-complete", data = { "total-number-of-presses-counted" = 2 } }
then = { action = "off", device = "desk_tape_light" }
"#;

    const START_UP_RULE: &str = r#"
version = 1
[[rules]]
name = "起動検知"
when = { device = "door_sensor", event = "start-up" }
then = { action = "on", device = "desk_tape_light" }
"#;

    #[test]
    fn matter_event_matches_on_name_alone_when_rule_has_no_data() {
        let file = rules(START_UP_RULE);
        let cfg = config_matter();
        // data 無しのイベント。
        assert!(matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_device_event(19, 0, "basicinformation", "start-up", None)
        ));
        // data 付きのイベントでも名前だけで一致する。
        assert!(matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_device_event(
                19,
                0,
                "basicinformation",
                "start-up",
                Some(serde_json::json!({"software-version": 3}))
            )
        ));
        // 名前違い。
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_device_event(19, 0, "basicinformation", "shut-down", None)
        ));
    }

    #[test]
    fn matter_event_data_is_partial_match_with_strict_types() {
        let file = rules(SWITCH_RULE);
        let cfg = config_matter();
        // 書いたキーだけ比較、他キー（previous-position）は無視。
        assert!(matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_device_event(
                25,
                2,
                "switch",
                "multi-press-complete",
                Some(serde_json::json!({"previous-position": 1, "total-number-of-presses-counted": 2}))
            )
        ));
        // 値違い（1 回押し）。
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_device_event(
                25,
                2,
                "switch",
                "multi-press-complete",
                Some(serde_json::json!({"total-number-of-presses-counted": 1}))
            )
        ));
        // 型違い（整数 2 vs 浮動小数 2.0）。
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_device_event(
                25,
                2,
                "switch",
                "multi-press-complete",
                Some(serde_json::json!({"total-number-of-presses-counted": 2.0}))
            )
        ));
        // ルールに data があるのにイベントに data が無い → 不一致。
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_device_event(25, 2, "switch", "multi-press-complete", None)
        ));
        // ルールのキーがイベントに無い → 不一致。
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_device_event(
                25,
                2,
                "switch",
                "multi-press-complete",
                Some(serde_json::json!({"previous-position": 1}))
            )
        ));
    }

    #[test]
    fn matter_event_priming_never_matches() {
        let file = rules(START_UP_RULE);
        let cfg = config_matter();
        let mut ev = mat_device_event(19, 0, "basicinformation", "start-up", None);
        ev.priming = true;
        // matd 再購読時の再配達（起動系イベントは特にまとめて来る）で発火してはならない。
        assert!(!matter_event_matches(&file.rules[0], &cfg, &ev));
    }

    #[test]
    fn matter_event_name_is_case_insensitive_and_accepts_numeric_id() {
        let cfg = config_matter();
        let file = rules(START_UP_RULE);
        assert!(matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_device_event(19, 0, "basicinformation", "Start-Up", None)
        ));
        // matd が ids テーブルに無いイベントを数値で流した場合、ルール側は 10 進表記で書く。
        let file = rules(
            r#"
version = 1
[[rules]]
name = "数値イベント"
when = { device = "door_sensor", event = "0" }
then = { action = "on", device = "desk_tape_light" }
"#,
        );
        let mut ev = mat_device_event(19, 0, "basicinformation", "start-up", None);
        ev.event = serde_json::json!(0);
        assert!(matter_event_matches(&file.rules[0], &cfg, &ev));
    }

    #[test]
    fn matter_event_source_must_match_node_and_configured_endpoint() {
        let cfg = config_matter();
        let file = rules(SWITCH_RULE);
        let press = |node, ep| {
            mat_device_event(
                node,
                ep,
                "switch",
                "multi-press-complete",
                Some(serde_json::json!({"total-number-of-presses-counted": 2})),
            )
        };
        assert!(matter_event_matches(&file.rules[0], &cfg, &press(25, 2)));
        // node 違い。
        assert!(!matter_event_matches(&file.rules[0], &cfg, &press(19, 2)));
        // wall_switch は endpoint = 2 指定なので endpoint 違いは不一致。
        assert!(!matter_event_matches(&file.rules[0], &cfg, &press(25, 1)));
        // door_sensor は endpoint 未指定なのでどの endpoint でも一致。
        let file = rules(START_UP_RULE);
        assert!(matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_device_event(19, 3, "basicinformation", "start-up", None)
        ));
        // 属性ルールにイベント行は一致しない（variant 違い）。
        let file = rules(MATTER_RULE);
        assert!(!matter_event_matches(
            &file.rules[0],
            &cfg,
            &mat_device_event(16, 1, "occupancysensing", "start-up", None)
        ));
    }

    #[test]
    fn due_matter_rules_fires_attribute_and_event_rules_from_mixed_batch() {
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
[[rules]]
name = "起動検知"
when = { device = "door_sensor", event = "start-up" }
then = { action = "on", device = "desk_tape_light" }
"#,
        );
        let batch = vec![
            mat::Incoming::Attribute(mat_event(16, 1, "occupancy", serde_json::json!(0))),
            mat::Incoming::Event(mat_device_event(19, 0, "basicinformation", "start-up", None)),
        ];
        let due = due_matter_rules(&file, &cfg, &batch, at(12, 0));
        let names: Vec<&str> = due.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["人感OFFで消灯", "起動検知"]);
    }
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p casad engine:: 2>&1 | grep -E "^error|cannot find" | head -5`
Expected: `matter_event_matches` が無い旨のコンパイルエラー。

- [ ] **Step 3: 突合を実装する**

`crates/casad/src/engine.rs` で、`matter_attribute_matches` と `attribute_matches` を次で置き換える:

```rust
/// Matter 属性トリガのルールが、与えられた mat listen の属性行 1 件に一致するか。
/// priming（matd 再購読時の現在値再配達）は状変ではないので無条件で不一致。
pub fn matter_attribute_matches(
    rule: &Rule,
    config: &Config,
    event: &mat::AttributeChange,
) -> bool {
    let Trigger::MatterAttribute {
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
    source_matches(config, device, event.node_id, event.endpoint)
        && name_matches(&event.attribute, attribute)
        && event.value == *equals
}

/// Matter デバイスイベントトリガのルールが、mat listen のイベント行 1 件に一致するか。
/// priming（matd 再購読時の再配達）は状変ではないので無条件で不一致。`data` は
/// 部分一致: ルールに書いたキーだけを JSON 値として型込みで等値比較し、書いていない
/// キーは無視する。ルールに `data` があるのに行に `data` が無ければ不一致。
pub fn matter_event_matches(rule: &Rule, config: &Config, event: &mat::MatterEvent) -> bool {
    let Trigger::MatterEvent {
        device,
        event: name,
        data,
    } = &rule.when
    else {
        return false;
    };
    if event.priming {
        return false;
    }
    if !source_matches(config, device, event.node_id, event.endpoint)
        || !name_matches(&event.event, name)
    {
        return false;
    }
    match data {
        None => true,
        Some(want) => match &event.data {
            None => false,
            Some(have) => want.iter().all(|(k, v)| have.get(k) == Some(v)),
        },
    }
}

/// ルールの発火元デバイス（matter・node_id 必須）と受信行の (node_id, endpoint) の突合。
/// devices.toml に `endpoint` があればそれも一致を要求する。
fn source_matches(config: &Config, device: &str, node_id: u64, endpoint: u64) -> bool {
    let (rule_node, rule_ep) = match config.device(device) {
        Ok(Device::Matter {
            node_id: Some(node_id),
            endpoint,
            ..
        }) => (node_id, endpoint),
        _ => return false,
    };
    if parse_node_id(rule_node) != Some(node_id) {
        return false;
    }
    match rule_ep {
        Some(ep) => u64::from(*ep) == endpoint,
        None => true,
    }
}

/// 受信行の名前（属性名 / イベント名。chip-tool 名 or 未知 ID の数値）とルール側の
/// 名前の突合。名前は case-insensitive、数値はルール側の 10 進表記と比較する。
fn name_matches(actual: &serde_json::Value, rule_name: &str) -> bool {
    match actual {
        serde_json::Value::String(s) => s.eq_ignore_ascii_case(rule_name),
        serde_json::Value::Number(n) => rule_name.trim().parse::<u64>().ok() == n.as_u64(),
        _ => false,
    }
}
```

`due_matter_rules` の match の `mat::Incoming::Event(_) => false` を `mat::Incoming::Event(ev) => matter_event_matches(r, config, ev)` にする。

- [ ] **Step 4: テスト・clippy が通ることを確認**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|error"`
Expected: すべて `test result: ok.`。

- [ ] **Step 5: コミット**

```bash
git add crates/casad/src/engine.rs
git commit -m "feat(casad): Matter デバイスイベント行をルールに突合して発火する

data は部分一致・型厳密、priming は不一致。--listen-once-mat は
属性行とイベント行の両方を 1 バッチで評価する。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01YVQ9U3CMVfuo12TkcxF73f"
```

---

### Task 5: `event_dedupe.rs` — event_number デデュープと常駐ループ配線

**Files:**
- Create: `crates/casad/src/event_dedupe.rs`
- Modify: `crates/casad/src/main.rs:1-8`（`mod event_dedupe;`）
- Modify: `crates/casad/src/engine.rs`（`matter_event_loop`）

**Interfaces:**
- Consumes: Task 2 の `mat::MatterEvent`
- Produces:
  ```rust
  pub struct EventWatermark;              // Default
  impl EventWatermark {
      pub fn new() -> Self;
      pub fn accept(&mut self, ev: &mat::MatterEvent) -> bool;   // true = 通す
      pub fn watermark_of(&self, ev: &mat::MatterEvent) -> Option<u64>;  // ログ用
  }
  ```

- [ ] **Step 1: 失敗するテストを含むモジュールを書く**

`crates/casad/src/event_dedupe.rs` を作成（実装は空のまま、テストだけ先に）:

```rust
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
        todo!()
    }

    /// ログ用: この行のキーに対する現在の watermark。
    pub fn watermark_of(&self, ev: &MatterEvent) -> Option<u64> {
        todo!()
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
```

`crates/casad/src/main.rs` の `mod` 群に `mod event_dedupe;` を追加（アルファベット順で `mod engine;` の後ろ、`mod enl;` の前ではなく `mod engine;` の次）:

```rust
mod dispatch;
mod engine;
mod enl;
mod event_dedupe;
mod mat;
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p casad event_dedupe:: 2>&1 | grep -E "panicked|not yet implemented|test result" | head -5`
Expected: `not yet implemented` で panic し FAILED。

- [ ] **Step 3: 実装する**

`event_dedupe.rs` の `impl EventWatermark` の 2 メソッドと、ヘルパを実装する:

```rust
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
```

- [ ] **Step 4: 常駐ループに配線する**

`crates/casad/src/engine.rs` の `use` に `use crate::event_dedupe::EventWatermark;` を追加し、`matter_event_loop` を次で置き換える:

```rust
/// Matter イベントリスナ。常駐 `mat listen --count 0` を 1 本張り、行ごとに一致ルールを
/// ワーカーに積む。one-shot の繰り返しでは再 spawn の空白で matd の broadcast から
/// こぼれる（prime バーストの recovered 昇格を取りこぼし、盲目期間の遷移が発火しない —
/// 2026-07-27 の書斎ライト事象）ため、ストリームを維持する。
/// ストリームが切れたら（matd 再起動・mat 不在等）バックオフして張り直す。
/// デバイスイベント行は `event_number` の単調性で重複配達を落とす（watermark は
/// 張り直しをまたいで保持する — 再購読の priming 再配達は既知の番号なので落ちる）。
fn matter_event_loop<'env>(
    file: &'env RuleFile,
    config: &Config,
    mat_bin: &str,
    dispatcher: &Dispatcher<'env>,
    cache: &StateCache,
) -> ! {
    let mut watermark = EventWatermark::new();
    loop {
        let result = mat::listen_stream(mat_bin, |incoming| {
            match &incoming {
                mat::Incoming::Attribute(ev) => {
                    // off no-op スキップ用の最終値キャッシュを先に更新する（priming 含む —
                    // 現在値の再配達はキャッシュには有効な知識）。
                    cache.record(ev);
                }
                mat::Incoming::Event(ev) => {
                    if !watermark.accept(ev) {
                        tracing::debug!(
                            node_id = ev.node_id,
                            endpoint = ev.endpoint,
                            cluster = %ev.cluster,
                            event = %ev.event,
                            event_number = ?ev.event_number,
                            watermark = ?watermark.watermark_of(ev),
                            "duplicate matter event dropped"
                        );
                        return;
                    }
                }
            }
            // 窓判定はイベント受信時刻で行う（ストリームは何時間も継続しうる）。
            let now = Local::now().time();
            let queued = dispatcher.dispatch_all(due_matter_rules(
                file,
                config,
                std::slice::from_ref(&incoming),
                now,
            ));
            if queued > 0 {
                tracing::debug!(queued, "matter event rule actions queued");
            }
        });
        match result {
            Ok(()) => tracing::warn!("mat listen stream ended; backing off"),
            Err(e) => tracing::warn!(error = %e, "mat listen failed; backing off"),
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}
```

- [ ] **Step 5: テスト・clippy が通ることを確認**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|error"`
Expected: すべて `test result: ok.`。

- [ ] **Step 6: コミット**

```bash
git add crates/casad/src/event_dedupe.rs crates/casad/src/main.rs crates/casad/src/engine.rs
git commit -m "feat(casad): Matter デバイスイベントの重複配達を event_number の単調性で落とす

(node_id, endpoint, cluster) ごとの watermark。start-up でその node を
リセットし、priming も watermark を更新する。常駐ストリームのみ適用。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01YVQ9U3CMVfuo12TkcxF73f"
```

---

### Task 6: ドキュメント・サンプル・バージョン 1.9.0

**Files:**
- Modify: `README.md`（191 行の Matter 行、429-452 の TOML 例、462-471 の active 段落、529 のデバッグコメント、537-556 の段落）
- Modify: `examples/rules.toml`
- Modify: `CLAUDE.md:212-217`
- Modify: `Cargo.toml:6`、`Cargo.lock`（`cargo build` で更新）

**Interfaces:** なし（ドキュメントのみ）。

- [ ] **Step 1: `examples/rules.toml` を更新する**

「Matter event trigger」のコメント行を「Matter attribute trigger」に変え、その `[[rules]]` ブロックの直後に追加する:

```toml
# Matter device event trigger (mat 1.36.0+): fire on an event line from mat listen
# (`event` key) instead of an attribute change. `data` is optional and a partial
# match: only the keys you write are compared (as JSON values, type included);
# other keys in the event are ignored. Omit `data` to match on the event name alone.
# `data = {}` is a config error.
[[rules]]
name = "example double press turns off the bedroom light"
when = { device = "wall_switch", event = "multi-press-complete", data = { "total-number-of-presses-counted" = 2 } }
then = { action = "off", device = "bedroom_light" }

[[rules]]
name = "example log when the sensor reboots"
when = { device = "door_sensor", event = "start-up" }
then = { action = "invoke", device = "bedroom_light", command = "color-temp", args = ["--kelvin", "2700"] }
```

`examples/devices.toml` に `wall_switch` / `door_sensor` は無いので、`power_strip_outlet2` の後ろにダミー node_id で追加する:

```toml
[devices.wall_switch]
protocol = "matter"
node_id = "25"
endpoint = 2

[devices.door_sensor]
protocol = "matter"
node_id = "19"
```

Run: `cargo run -q -p casad -- check examples/rules.toml --config examples/devices.toml > /dev/null; echo exit=$?`
Expected: `exit=0`

- [ ] **Step 2: README を更新する**

1. 191 行目の Matter 行の末尾（`— casad 1.8.1+ recognizes and ignores them at debug level (no rule syntax for event triggers yet)`）を `— casad 1.9.0+ fires \`event\` triggers on them (1.8.1 only recognized and ignored them)` に置き換える。
2. 429 行目のコメント `# Matter event trigger: when study_motion's occupancy becomes 0 (vacant), turn off the desk light` を `# Matter attribute trigger: when study_motion's occupancy becomes 0 (vacant), turn off the desk light` に変え、`active` の例ブロック（`then   = { action = "on", device = "study_fan" }`）の直後・コードフェンス終了の前に追加する:

```toml

# Matter device event trigger (mat 1.36.0+): a Generic Switch double press turns
# the living light off. `data` is a partial match on the event's fields.
[[rules]]
name = "living light off on double press"
when = { device = "wall_switch", event = "multi-press-complete", data = { "total-number-of-presses-counted" = 2 } }
then = { action = "off", device = "living_light" }

# The same door sensor as an event trigger instead of an attribute trigger.
[[rules]]
name = "closet light on when the door opens"
when = { device = "closet_door", event = "state-change", data = { "state-value" = true } }
then = { action = "on", device = "closet_light" }
```

3. 469 行目付近 `The window applies to every trigger kind — time, ECHONET event, and Matter event —` を `The window applies to every trigger kind — time, ECHONET event, Matter attribute, and Matter device event —` にする。
4. 529 行目 `# Debug: run mat listen exactly once to evaluate Matter event triggers` を `# Debug: run mat listen exactly once to evaluate Matter attribute / device event triggers` にする。
5. 537 行目からの段落で `for \`attribute\` triggers.` を `for \`attribute\` and \`event\` triggers.` にし、段落末尾（`(\`CASA_ENL_BIN\` / \`CASA_MAT_BIN\` / \`[binaries]\` / \`PATH\`).`）の直後に新しい段落を追加する:

```markdown

#### Matter device event triggers (`event`)

With mat 1.36.0+ the `mat listen` stream also carries device-originated
*event* lines (`event` key instead of `attribute`): Generic Switch presses
(`initial-press`, `multi-press-complete`, ...), `booleanstate`'s `state-change`,
BasicInformation's `start-up`, and so on. `when = { device, event, data? }`
fires on them:

- `device` must be a Matter device with a `node_id` (groupcast devices are
  rejected by `casad check`); a configured `endpoint` narrows the match, as for
  attribute triggers.
- `event` is the event name in chip-tool notation, compared case-insensitively;
  an event mat does not know is reported as its numeric id, which you can write
  as a decimal string (`event = "0"`).
- `data` is optional and a **partial match**: every key you write must be
  present in the event's `data` with an equal JSON value (type included — `2`
  does not match `2.0`); keys you do not write are ignored. Omit `data` to fire
  on the event name alone. A rule with `data` never matches an event line that
  carries no `data` at all, and `data = {}` is a config error.
- `priming: true` event lines (matd's redelivery on (re)subscribe — start-up
  events come in a burst of dozens after a matd restart) never fire, same as
  attributes.
- The resident stream de-duplicates event lines by Matter's `event_number`:
  per (node, endpoint, cluster), a line whose number is not greater than the
  last one seen is dropped at debug level. A `start-up` event resets that
  node's watermarks (a factory reset restarts the counter). The watermark is
  in-memory only and `--listen-once-mat` does not apply it.
- An event-triggered `off` gets the same `off_grace_secs` treatment as other
  event-triggered `off`s.
```

- [ ] **Step 3: CLAUDE.md の casad 責務を更新する**

212-217 行の箇条書き（`Time triggers (internal scheduler) / event triggers (…)`）の末尾 `\`recovered\` events fire normally)` を次に置き換える:

```
    `recovered` events fire normally). With mat 1.36.0+ the same stream carries
    Matter device *event* lines (`event` key); `when = { device, event, data? }`
    fires on them (`data` is a partial, type-strict match; `priming` never fires;
    the resident stream de-duplicates by `event_number` per (node, endpoint,
    cluster) with a `start-up` reset)
```

- [ ] **Step 4: バージョンを 1.9.0 にする**

```bash
sed -i 's/^version = "1.8.1"$/version = "1.9.0"/' Cargo.toml
cargo build --workspace 2>&1 | tail -1     # Cargo.lock を更新
grep -c '1.9.0' Cargo.lock                 # 3 のはず（casa / casa-core / casad）
```

- [ ] **Step 5: 全体検証**

Run: `cargo fmt --all -- --check && cargo clippy --workspace -- -D warnings && cargo test --workspace 2>&1 | grep -E "^test result|FAILED|error"; cargo run -q -p casad -- check examples/rules.toml --config examples/devices.toml >/dev/null && echo check-ok`
Expected: fmt 差分なし、clippy エラーなし、すべて `test result: ok.`、`check-ok`。

- [ ] **Step 6: コミット**

```bash
git add README.md examples/rules.toml examples/devices.toml CLAUDE.md Cargo.toml Cargo.lock
git commit -m "docs: Matter デバイスイベントトリガ（event / data）を README・例・CLAUDE.md に記載 (1.9.0)

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01YVQ9U3CMVfuo12TkcxF73f"
```

---

## 実装後（この計画の範囲外）

spec の「実機確認手順」に従い、hogar への配布は despliegue skill で別途行う。スイッチの node_id / endpoint / `data` の実値は hogar で `mat listen --event --count 0` を張って採取する。
