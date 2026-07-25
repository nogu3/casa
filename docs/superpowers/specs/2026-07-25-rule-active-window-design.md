# ルールの有効時間帯（`active` ウィンドウ）対応

- 日付: 2026-07-25
- 対象: casad（rules / engine / cli）、jarvis 実機 config（devices.toml / rules.toml）
- version bump: workspace `1.3.0` → `1.4.0`（新機能・minor）

## 背景と課題

書斎に扇風機（SwitchBot Hub 2 の赤外線リモコン "扇風機(BARREL)" / `remoteType = DIY Fan`）を
導入した。これを書斎の人感センサー `study_motion`（Matter node 16 / occupancysensing）に
連動させたい。要件は次の 3 つ。

1. 書斎が**不在**（occupancy = 0）になったら扇風機を ON にする（空気を回す）
2. 書斎が**在室**（occupancy = 1）になったら扇風機を OFF にする（在室中は音がうるさい）
3. **21 時以降はこの連動を止める**。かつ 21 時に扇風機が回っていたら止める（就寝時の騒音回避）

1 と 2 は既存の Matter イベントトリガでそのまま書ける。3 のうち「21 時に止める」も
既存の時刻トリガで書ける。**書けないのは「21 時以降は連動そのものを止める」**。

現状の rules DSL の `when` は「イベント（echonet EPC / matter attribute）」か「時刻」の
排他二択で、**ルールを時間帯で有効化／無効化する手段が無い**。したがって casad に
機能追加が要る。

なお同じ `study_motion` には既に書斎照明の点灯／消灯ルールがぶら下がっており、
**照明は夜間も人感で動いてほしい**。よって「センサー単位で夜間止める」のではなく
**ルール単位で有効時間帯を持たせる**必要がある。

## 検討した方式

| 方式 | 内容 | 判定 |
|---|---|---|
| **A. ルール単位の `active` フィールド** | `Rule` に `Option<ActiveWindow>` を足し、マッチ時に窓外を除外する | **採用** |
| B. `when` の中に時間帯を入れる | `when = { device = ..., equals = 0, active = {...} }` | 却下。`Trigger` は untagged enum で、全 variant にオプショナルフィールドを足すとパース失敗時のエラーが潰れる。本 repo が `Thens` で手書き `Deserialize` までして避けてきた問題を持ち込む |
| C. 名前付きスケジュールを定義して参照 | `[schedules.daytime]` を `[[rules]]` から参照 | 却下（YAGNI）。複数ルールでの窓の共有は魅力だが間接参照と検証が増える。A の構文はそのまま残せるので、必要になってから重ねられる |
| D. casad を改修しない | 21:00 / 06:00 に rutinas から rules.toml を差し替えて casad を restart | 却下。restart で `enl listen` / `mat listen` の購読が切れ、洗面照明・植物ライトのルールも巻き添えで止まる。状態を casad の外に持つことになり設計思想にも反する |

## 設計

### DSL 表面

`Rule` にオプショナルフィールドを 1 本足す。`Trigger` の untagged enum には触らない。

```toml
[[rules]]
name   = "書斎 不在で扇風機ON"
when   = { device = "study_motion", attribute = "occupancy", equals = 0 }
active = { from = "06:00", to = "21:00" }   # ← 追加。省略時は常時有効
then   = { action = "on", device = "study_fan" }
```

キー名は `active`、値は**インラインテーブル `{ from, to }`**。既存の `when` / `then` が
すべてインラインテーブルなので表記が揃う。`from` / `to` は必須（片側省略は不可）。

### 型

```rust
pub struct Rule {
    pub name: String,
    pub when: Trigger,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<ActiveWindow>,
    pub then: Thens,
}

pub struct ActiveWindow {
    pub from: String,   // "HH:MM"
    pub to: String,     // "HH:MM"
}
```

`from` / `to` は既存の `Trigger::Time { at }` と同じく **String のまま保持**し、HH:MM の
検証は `engine::validate_schedule` が担う（パース責務の置き場所を既存と揃える）。

`skip_serializing_if = "Option::is_none"` により、`active` を持たないルールの
`casad check` JSON 出力は現状と完全に一致する（後方互換）。

### 意味論

| 条件 | 挙動 |
|---|---|
| `active` 未指定 | 常時有効（既存ルールは無改修で従来どおり） |
| `from < to` | 同日内の窓。例 `{ from = "06:00", to = "21:00" }` |
| `from > to` | 日跨ぎ。例 `{ from = "21:00", to = "06:00" }` → 21:00〜23:59 と 00:00〜05:59 |
| `from == to` | **エラー**（全日とも空区間とも読めるため `config_parse` / exit 10 で弾く） |

**区間は `from` を含み `to` を含まない半開区間 [from, to)**。

この半開区間が今回の要件に直結している。21:00 ちょうどにイベントルールは既に無効で、
同じ 21:00 の分境界で時刻トリガの OFF が撃たれる、という順序が定義から自然に決まる。

判定は `chrono::NaiveTime` の比較で行う。時刻トリガが分粒度で回るのに対し窓判定は
秒粒度だが、境界が分単位でしか指定できない以上、実挙動の差は生じない。

### 適用箇所

窓の判定はマッチ関数 3 つに集約する（発火経路に条件が散らばらない）。

- `due_time_rules(file, now)` — 既に `now: NaiveTime` を受け取っている
- `due_event_rules(file, config, events, now)` — `now` 引数を追加
- `due_matter_event_rules(file, config, events, now)` — `now` 引数を追加

呼び出し元（`event_loop` / `matter_event_loop` / `drain_events_once` /
`drain_matter_events_once` / `fire_due_events`）は `Local::now().time()` を渡す。
純粋関数に時刻を注入する形なので、テストから固定時刻を渡せる。

窓外でルールを落とした場合は `tracing::debug!` に残す（発火しない理由が journal から
追えるようにする。「センサーは反応しているのに動かない」の切り分けコストを下げる）。

### CLI

`--now` の `requires = "once"` 制約を外し、`--listen-once` / `--listen-once-mat` でも
使えるようにする。21 時を待たずに「窓外ならイベントが来ても発火しない」を実機で
検証するために要る。`--now` は引き続き `--once` 系デバッグ経路専用で、常駐時
（フラグなしの `casad run`）には影響しない。

### 検証

`engine::validate_schedule` を拡張し、各ルールの `active.from` / `active.to` の HH:MM 書式と
`from != to` を検証する。エラーにはルール名を添える既存の形式を踏襲する。
`casad check` と `casad run` 起動時の両方がこの検証を通るので、不正な窓を持つルールで
常駐が始まることはない。

rules.toml の `version` は **1 のまま**（オプショナルフィールドの追加＝後方互換）。

## 実機設定

### devices.toml（jarvis-iac `roles/casa/files/devices.toml`）

```toml
# 書斎の扇風機。Hub 2 に赤外線リモコン "扇風機(BARREL)" (DIY Fan) として登録。
[devices.study_fan]
protocol = "switchbot"
device_id = "01-202607251040-40751692"
```

DIY 系の赤外線リモコンなので SwitchBot クラウド API の標準コマンド `turnOn` / `turnOff` が
通り、casa の `on` / `off` に直結する（既に稼働している `plant_light` = DIY Light と同方式）。

### rules.toml（jarvis-iac `roles/casa/files/rules.toml`）

既存の「書斎 人感ONで点灯 / 人感OFFで消灯」は**変更しない**（照明は夜間も人感で動く）。
以下 3 ルールを追加する。

```toml
# 書斎に人がいない間だけ扇風機で空気を回す。在室中と 21 時以降は音がうるさいので止める。
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
# 既に止まっているときに OFF を送っても無害。
[[rules]]
name = "扇風機 21時消灯"
when = { at = "21:00" }
then = { action = "off", device = "study_fan" }
```

21 時の停止を無条件送信にできる根拠は、この扇風機のリモコンが **電源 ON と OFF で
別の赤外線コード**を持つこと（トグル 1 ボタンではない）。停止中に OFF を送っても
何も起きない。

## テスト

### 単体テスト（`crates/casad/src/rules.rs` / `engine.rs`）

- `active` のパース: 正常 / `from` が不正な HH:MM / `to` が不正な HH:MM / `from == to` / 未指定
- 窓判定: 内側 / `from` 境界は含む / `to` 境界は含まない / 外側 / 日跨ぎ窓の両側（夜側・朝側）/ 日跨ぎ窓の窓外 / `active` 未指定は常に true
- `due_time_rules` が窓外の時刻ルールを除外する
- `due_event_rules`（echonet）が窓外を除外し、窓内では従来どおり発火する
- `due_matter_event_rules`（matter）が窓外を除外し、窓内では従来どおり発火する
- 後方互換: `active` を持たないルールが任意の時刻で発火する

### 統合テスト（`crates/casad/tests/`）

- `casad check` が `active` 付き rules.toml を受理し、JSON に `active` を含めて出す
- `active` を持たないルールの `casad check` JSON に `active` キーが現れない
- 不正な `active`（HH:MM 書式違反 / `from == to`）で exit 10 と構造化エラー

### 実機検証（jarvis）

- `casad check ~/.config/casa/rules.toml` が通る
- `casad run rules.toml --listen-once-mat --now 22:00` — 窓外。人感イベントを起こしても扇風機ルールは発火しない（照明ルールは発火する）
- `casad run rules.toml --listen-once-mat --now 12:00` — 窓内。不在で扇風機が回り、在室で止まる
- `casad run rules.toml --once --now 21:00` — 扇風機に OFF が飛ぶ
- 実機で扇風機（BARREL）が実際に回る／止まることを目視確認する

## 配布

変更は **casad のみ**（`casa-core` も `casa(bin)` も無改修）。

1. casad を aarch64 へクロスビルドし jarvis へ配布（despliegue の手順）
2. jarvis-iac の `roles/casa/files/devices.toml` / `rules.toml` を更新
3. `ansible-playbook site.yml --check --diff` で差分確認 → 本適用（casad が restart される）
4. 上記の実機検証

## ドキュメント

- README の casad セクションに `active` の説明と例を追加
- `examples/rules.toml` に `active` 付きルールの例を追加
- CLAUDE.md の casad 責務の記述に「ルールの有効時間帯」を追記

## 既知の制約（本件では対応しない）

`study_motion`（node 16）は keep-alive を一切送らないため、matd の購読が切れると次の
在室変化まで状態を取り戻せない。これは既存の書斎照明ルールが既に抱えているのと同じ
露出で、対策は matd 側の priming 差分回復（別件・spec 済）。

窓が開いた瞬間（06:00）にセンサーの現在値を取りに行って状態を揃える、といった
「窓の境界での状態同期」は行わない。06:00 以降、最初の在室変化イベントまで扇風機は
21:00 に止めたままの状態を保つ。
