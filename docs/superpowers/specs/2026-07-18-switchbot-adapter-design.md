# SwitchBot アダプタ（クラウド制御）設計

- 日付: 2026-07-18
- 対象: casa Phase 4 の SwitchBot 対応（残る唯一の未対応プロトコル）
- 前提バイナリ: 自作 `swb`（`~/ghq/github.com/nogu3/swb`）。**公式 CLI ではない**（CLAUDE.md の「公式 CLI 利用」記述は古く、本設計で更新する）。

## 背景と目的

casa の Phase 0〜4（Matter `mat` まで）は実装済みで、残る唯一の未対応プロトコルが
SwitchBot。かつては「公式 CLI 完成待ち」だったが、自作 `swb` が `scan` / `models` /
`devices` / `status` / `cmd` を実装済みでアンブロックされた。

`swb` には 2 つの平面がある:

- **BLE パッシブスキャン**（`scan --once/--follow`）— センサ読み取り（Meter, Hub2 等）。ハブ・クラウド不要。
- **クラウド API v1.1**（`status <device>` / `cmd <device> <command> --param`）— 制御と状態取得。

本設計は**クラウド制御平面を対象**とする（BLE センサ読みは将来の別作業）。

## 核心の写像判断

SwitchBot クラウド API には **単一プロパティ読み取りが存在しない**（`status` は全状態を
一括で返す GET のみ）。ECHONET の EPC・Matter の attribute のような単一セレクタ read が
プロトコル上のプリミティブとして無い。

したがって casa の `get <name> <property>`（`property` は CLI 上必須）は SwitchBot では
**未対応**（`protocol_unsupported`, exit 14）とする。property を無視して status 全体を
返す案・casa 側で status から property を抽出する案は退けた。前者は必須引数が無意味になり
誤解を招き、後者は「アダプタは引数を組むだけ・ops はプロトコル非依存」というアーキテクチャを
壊す（抽出は adapter では組めず ops に switchbot 固有の後処理が入る）。

読み取りは `casa invoke <name> status` で全状態を取得する経路を正とする。これは
「casa に無いプロトコルセマンティクスを偽装しない」という設計原則 1 に忠実。

## アダプタ実装

新規ファイル: `crates/casa-core/src/adapter/switchbot.rs`

- 構造体 `SwitchbotAdapter`、バイナリ名 `const BIN: &str = "swb"`。
- `address(device)` は `Device::Switchbot { device_id }` から `device_id` を取り出すヘルパ。
- 実装するのは `power` と `invoke` の 2 メソッドのみ。`get`/`set`/`describe` は trait 既定の
  `None` のまま（= 未対応）。

| casa 操作 | swb 呼び出し | メソッド |
|---|---|---|
| `on <name>` | `swb cmd <device_id> turnOn` | `power(on=true)` |
| `off <name>` | `swb cmd <device_id> turnOff` | `power(on=false)` |
| `invoke <name> <command> [args...]` | `swb <command> <device_id> [args...]` | `invoke` |
| `get` / `set` / `describe` | （未実装） | 既定 `None` → exit 14 |

### address 注入の一様性（invoke trait の前提）

invoke trait は「アドレス注入がコマンドによらずプロトコルごとに一様」であることを前提とする。
swb は `status <device>` / `cmd <device> <command>` と、device_id が**常にサブコマンド直後の
第 1 位置引数**に来る。よって invoke は `[command, device_id, ...args]` を組む。Matter が
`--node <id>` フラグ注入だったのと対照的に、swb は**位置引数注入**。

`scan`/`models`/`devices` は device を取らないため `casa invoke plug scan` のような誤用は
`swb scan <device_id>` となり swb 側でエラー→ exit code 伝播で可視化される（Matter アダプタで
`--endpoint` を取らないコマンドに対する挙動と同じ扱い）。

### 具体例

- `casa on plug` → `swb cmd <id> turnOn`
- `casa off plug` → `swb cmd <id> turnOff`
- `casa invoke plug status` → `swb status <id>`
- `casa invoke plug cmd setBrightness --param 50` → `swb cmd <id> setBrightness --param 50`

## dispatch の変更

`crates/casa-core/src/adapter/mod.rs`:

- `pub mod switchbot;` を追加。
- `adapter_for` の `Device::Switchbot { .. } => None` を
  `Some(&switchbot::SwitchbotAdapter)` に変更。
- 直上の Phase 4 コメント（「公式 switchbot CLI（@switchbot/openapi-cli）を呼ぶ」）を
  「自作 `swb` を呼ぶ」に更新。

## 認証

casa は認証情報を一切扱わない。`SWITCHBOT_TOKEN`/`SWITCHBOT_SECRET` は swb 側の責務で、
子プロセス spawn 時に casa の環境変数がそのまま継承される。CLAUDE.md の方針どおり、casa の
config / 環境変数から何も渡さない。

## config スキーマ

**変更なし**。`Device::Switchbot { device_id }` は既に存在する。roadmap どおり、新プロトコル
追加で subcommand も config スキーマも変わらない（`protocol` の値が既存なだけ）。

## 既存テストへの波及

全 3 プロトコル（echonet / matter / switchbot）がアダプタを持つことになるため、
「アダプタ未実装」を前提にした既存テスト 2 件を書き換える:

1. `adapter/mod.rs::switchbot_has_no_adapter_yet`
   → `switchbot_devices_dispatch_to_switchbot_adapter` に変更。
   `adapter_for(&switchbot_device).unwrap().power(...).bin == "swb"` を検証。

2. `ops.rs::validate_reports_summary_and_flags_protocols_without_adapter`
   → switchbot を no_adapter の例に使っているので書き換える。全デバイスにアダプタがある構成で
   `warnings` が空（no_adapter 警告ゼロ）になることを検証する形にする。

`validate` の no_adapter 生成コードパス自体は将来プロトコル用の防御として**残す**（enum に
variant を足したがアダプタ未実装、という段階を可視化するため）。ただし実 `Device` variant では
発火不能になる点を許容する。

## 新規テスト

`switchbot.rs` の `#[cfg(test)]`:

- `power(on=true)` → `["cmd", "<id>", "turnOn"]`
- `power(on=false)` → `["cmd", "<id>", "turnOff"]`
- `invoke("status", [])` → `["status", "<id>"]`
- `invoke("cmd", ["turnOn"])` → `["cmd", "<id>", "turnOn"]`
- `invoke("cmd", ["setBrightness", "--param", "50"])` → 追加引数が素通しされる
- `get`/`set`/`describe` が `None`
- 全ケースで `bin == "swb"`

device_id はダミー値（例 `"DUMMY-XX-XX"`）を使う（公開リポジトリ・実 ID 禁止）。

## ドキュメント

- `README.md`:
  - 兄弟 CLI 表の SwitchBot 行を「self-authored `swb`」に更新。
  - on/off 対応表に switchbot（on/off = `swb cmd turnOn`/`turnOff`）を追加。
  - `get`/`set`/`describe` は switchbot 未対応で、読み取りは `casa invoke <name> status` 経由と明記。
  - 最低 `swb` バージョンを記載（現行 `0.1.0`。`status` / `cmd` サブコマンドと exit code 規約を前提とする）。
- `CLAUDE.md`: Phase 4 の SwitchBot 節を「自作 `swb` を呼ぶ」に更新（公式 CLI 記述は削除）。
- version bump: ワークスペース `0.7.1` → `0.8.0`（新プロトコル対応）。

## 受け入れ基準

- `cargo build` / `cargo test` / `cargo clippy -- -D warnings` が通る。
- 新規アダプタ単体テストが全て通る。
- 書き換えた既存テスト 2 件が通る。
- E2E 実機テストは CI では回さず README に手順を残す（既存 enl/mat と同じ方針）。

## スコープ外

- BLE スキャン平面（`scan`）の casa 統合。将来の別作業。
- `set`/`describe`/`get` の switchbot 対応（プロトコルに単一プロパティ read/write が無いため恒久的に未対応の可能性が高い）。
- config スキーマ変更。
- casad 側の変更（switchbot デバイスは既存の `run`/`check` 経路で casa 経由で操作可能になり、casad 固有の変更は不要）。
