# casad Matter デバイスイベントトリガ対応 — 設計

日付: 2026-09-10
状態: 承認済み

## 目的

casad のルール DSL に、`mat listen` が流す **Matter デバイスイベント行**（`event` キー、mat 1.36.0+）をトリガにする構文を追加する。現状 casad は event 行を `MatterEvent` にパースして debug ログを残すだけで発火しない（casad 1.8.1、`crates/casad/src/mat.rs`）。

対象ユースケース（3 つとも対象）:

1. Generic Switch の押下（`initial-press` / `multi-press-complete` の `data.total-number-of-presses-counted`）で照明を操作する。
2. `booleanstate` の `state-change` を、現行の属性トリガ（`attribute = "state-value"`）の代わりに使う（廊下収納ドア、node 19）。
3. 起動系イベント（`start-up` / `boot-reason` 等）の検知。

## 背景と前提（調査済み）

- 本番 hogar-matd は 2026-09-10 から全クラスタのイベントを wildcard 購読しており、起動系（start-up / boot-reason / access-control-entry-changed）や switch の initial-press / multi-press-complete が流れる。
- event 行の形（mat docs/commands.md「mat listen」節）:
  ```json
  {"timestamp":"…","node_id":25,"endpoint":2,"cluster":"switch","event":"initial-press","event_number":1725600000123,"priority":"info","data":{"new-position":1},"priming":false}
  {"timestamp":"…","node_id":25,"endpoint":2,"cluster":"switch","event":"multi-press-complete","event_number":1725600000130,"priority":"info","data":{"previous-position":1,"total-number-of-presses-counted":2},"priming":false}
  {"timestamp":"…","node_id":19,"endpoint":1,"cluster":"booleanstate","event":"state-change","event_number":590151,"priority":"info","data":{"state-value":false},"device_time":{"system_ms":577001426},"priming":true}
  ```
  - `cluster` / `event` は ids テーブルの名前、未知なら数値。
  - `data` はイベントフィールドの kebab 名キーのオブジェクト。未知フィールドは 10 進コンテキストタグをキーにする。イベントがデータを持たなければ **キー自体が無い**。
  - `priming: true` は matd の（再）購読時の再配達で、状変ではない。
  - event 行に `recovered` キーは無い（matd が EventNumber で厳密に回収するため）。
- 既存の属性トリガ（`when = { device, attribute, equals }`）は動作中で、コード上は `Trigger::MatterEvent`、README では「Matter event trigger」と呼ばれている。本物のイベントトリガと名前が衝突する。

## 設計判断（ユーザー合意済み）

| 論点 | 決定 |
|---|---|
| `when` 構文 | `event` + 任意の `data` 部分一致。`cluster` は書かない |
| `priming: true` の event 行 | 属性と同じく無条件で発火しない |
| 同一イベントの重複配達 | (node_id, endpoint, cluster) ごとに最大 `event_number` を記憶し、単調性で落とす |
| 命名 | 既存の属性トリガを `MatterAttribute` に改名し、`MatterEvent` を本物のイベントトリガに当てる。TOML 構文は不変 |
| 実機確認 | 廊下収納ドア（node 19）の state-change と、実機のある Matter スイッチ（node / endpoint は hogar のログで採取） |

## DSL

```toml
# スイッチ 2 回押しでリビング消灯
[[rules]]
name = "wall switch double press -> living off"
when = { device = "living_switch", event = "multi-press-complete", data = { "total-number-of-presses-counted" = 2 } }
then = { action = "off", device = "living_light" }

# ドアが開いたら点灯（属性トリガの event 版）
[[rules]]
name = "door opened"
when = { device = "hallway_closet_door_sensor", event = "state-change", data = { "state-value" = true } }
then = { action = "on", device = "hallway_closet_light" }

# 起動検知（data 条件なし）
[[rules]]
name = "sensor rebooted"
when = { device = "hallway_closet_door_sensor", event = "start-up" }
then = { action = "invoke", device = "some_device", command = "some-command" }
```

意味論:

- `device` は devices.toml の **matter かつ `node_id` を持つ**デバイス（group 不可）。既存 `check_matter_device` を再利用する。devices.toml に `endpoint` があればイベントの `endpoint` も一致を要求する（属性トリガと同じ）。
- `event` はイベント名。突合は属性と同じ規則: 文字列なら大文字小文字を無視して比較、mat が未知 ID を数値で出した場合はルール側の 10 進表記と比較する。
- `data` は省略可能な TOML テーブル。**部分一致**: ルールに書いたキーだけをイベントの `data` と JSON 値として型込みで等値比較し（`equals` と同じく `2` と `2.0` は不一致）、ルールに無いキーは無視する。省略時はイベント名だけで発火する。
- ルールに `data` があり、イベント行に `data` キーが無ければ不一致。
- `data = {}`（空テーブル）は設定エラー（`casad check` / `run` が拒否）。書き間違いで無条件発火するのを防ぐ。
- `priming: true` の行は無条件で不一致。
- `active` 窓は他のトリガ種別と同じく受信時刻で判定する。

## casad コンポーネント

### `rules.rs`

- `Trigger::MatterEvent { device, attribute, equals }` を `Trigger::MatterAttribute` に改名。
- 新 variant `Trigger::MatterEvent { device: String, event: String, data: Option<serde_json::Map<String, serde_json::Value>> }`。untagged enum の判別キーは `epc` / `attribute` / `event` / `at` で互いに素。
- `RuleFile::validate` で `MatterEvent` にも `check_matter_device` を適用する。`data = {}` は `parse`（`then` 空の検査と同じ場所）で `ConfigParse` エラーにする（`check` / `run` とも通る）。

### `mat.rs`

- 属性行の `Event` を `AttributeChange` に改名。
- `MatterEvent` に `event_number: Option<u64>` と `data: Option<serde_json::Map<String, serde_json::Value>>` を追加。どちらも欠落は `None`（`#[serde(default)]`）。
- 公開 enum `Incoming { Attribute(AttributeChange), Event(MatterEvent) }` を追加。`listen_stream` のコールバック引数と `listen_once` の戻り値（`Vec<Incoming>`）をこれにする。分類は既存 `classify_line` のまま。パース不能行は従来どおり warn で skip。
- 受信した event 行は全件 debug ログ（node_id / endpoint / cluster / event / event_number / data / priming）。

### `engine.rs`

- 属性用の `matter_event_matches` を `matter_attribute_matches` に改名。`due_matter_event_rules` は `due_matter_rules(file, config, &[Incoming], now)` にして属性行・イベント行の混在バッチを 1 関数で突合する。
- イベント用 `matter_event_matches(rule, config, &MatterEvent) -> bool` を追加。発火元 (node_id, endpoint) の突合は `source_matches`、名前突合は `name_matches` として属性と共有する。
- `matter_event_loop`: `Incoming::Attribute` → 従来処理（state cache 更新 + 属性ルール）、`Incoming::Event` → デデュープ → イベントルール発火。デデュープ状態はループが所有する（ストリームスレッドだけが触るので Mutex 不要）。
- `drain_matter_events_once`（`--listen-once-mat`）: 1 バッチで属性・イベント両方のルールを評価する。デデュープは off-grace / min-gap と同じくワーカーを通らない同期パスには適用しない。
- `has_matter_events` の判定に両 variant を含める。
- dispatch / state cache / off-grace / no-op スキップは変更なし（`then` 側は共通）。

### `event_dedupe.rs`（新規）

`EventWatermark`: `HashMap<(u64, u64, String), u64>`（キー = node_id, endpoint, cluster の文字列表現）。

- `accept(&mut self, ev: &MatterEvent) -> bool`
  - `event_number` が `None` → 常に true（watermark は更新しない）。
  - `priming: false` かつ cluster が BasicInformation かつ `event` が `start-up` → その node_id のキーを全部クリアしてから、自身の番号で watermark を設定し true。カウンタ巻き戻し（工場リセット等）対策。
  - `event_number <= watermark` → false（呼び側が debug ログで捨てる）。
  - それ以外 → watermark を更新して true。
- `priming: true` の行も `accept` を通す（watermark を更新する）。発火しないのは `matter_event_matches` 側の責務。
- in-memory のみ。casad 再起動で消える（CLAUDE.md「状態を持たない」方針の範囲内: 属性の state cache と同じ扱い）。

## ログ

- event 行受信: debug（全件）。
- 重複で捨てた行: debug（`duplicate matter event dropped`、event_number と watermark を含む）。
- 発火: 既存の dispatch ログ（変更なし）。

## ドキュメント・バージョン

- README「casad」節: 既存の「Matter event trigger」の記述を「Matter attribute trigger」に改名し、「Matter event trigger」節を新設（構文、部分一致・型厳密、priming、デデュープと `start-up` リセット、mat 1.36.0+ 必須、`--listen-once-mat` が両方を評価すること）。
- `examples/rules.toml` にイベントトリガの例を追加（ダミー値）。
- CLAUDE.md の casad 責務の箇条書き（イベントトリガの項）を更新。
- workspace version 1.8.1 → 1.9.0。

## 既知の割り切り

- `event_number` の巻き戻りは `start-up` イベントでのみ検出する。`start-up` が届かずにカウンタが戻った場合、その node のイベントは番号が watermark を超えるまで捨てられる（casad 再起動で復旧）。実機で観測されたら改めて扱う。
- `priming: true` の `start-up` は watermark をリセットしない（matd 再起動のたびに全 watermark が消えるのを避ける）。ストリーム断中に工場リセットされた node は、live の start-up が来ない限り番号が watermark を超えるまで捨てられる（casad 再起動で復旧）。
- デデュープキーが cluster 単位なので、matd が異クラスタ間で順序を入れ替えて配っても取りこぼさない。同一クラスタ内での順序入れ替えは想定しない。
- `data` のキー名は検証しない（イベントごとのスキーマを casad は知らない）。

## テスト（TDD）

- `rules.rs`: event トリガのパース（`data` あり / なし）、`data = {}` のエラー、`epc` / `attribute` / `event` / `at` の判別が崩れないこと、group デバイス拒否。
- `mat.rs`: 実ログ形式のフィクスチャ（switch 2 種・booleanstate・start-up）を `Incoming` に分類、`data` 欠落 → `None`、`event_number` 欠落 → `None`。
- `engine.rs`: 名前一致（大文字小文字・数値 ID）、`data` 部分一致、型不一致（`2` vs `2.0`）、priming 不一致、endpoint 絞り込み、`data` ルール vs `data` 無しイベント、node 不一致、混在バッチ（`listen_once`）で属性・イベント両方のルールが発火。
- `event_dedupe.rs`: 単調で落ちる、キー独立、priming で watermark 更新、`start-up` で node 単位リセット、`event_number` 欠落は通す。
- 既存テストは改名に追従するだけで意味は変えない。

## 実機確認手順（実装完了後、配布は despliegue skill）

1. hogar で `mat listen --event --count 0` を張り、ドア開閉とスイッチ押下で event 行を採取する（スイッチの node_id / endpoint / `data` の実値を確定）。
2. hogar-iac `hogar/config/casa/rules.toml` のドアのルールを event 版に置き換え、スイッチのルールを追加する。配布前に `casad check` で exit 0 を確認する。
3. `RUST_LOG=debug` で「event received」「duplicate matter event dropped」「発火」のログを確認し、ドア開閉とスイッチ押下で照明が動くこと。
4. matd を再起動し、priming バーストで何も発火しないこと（アクション実行ログが出ないこと）を確認する。
