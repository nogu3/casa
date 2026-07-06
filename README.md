# casa

スマートホーム横断クライアント。プロトコル固有 CLI（`enl` 等）をサブプロセスで呼び、
人間に優しい名前のマッピングと統一された CLI UX を提供する。

casa はプロトコルを直接喋らない。バイト列もソケットも持たず、すべて兄弟 CLI に委譲する。
設計の全体像は [CLAUDE.md](CLAUDE.md) を参照。

## 使い方

```bash
# 設定済みデバイスの一覧
casa list

# プロパティの読み取り（ECHONET Lite なら EPC 指定。enl をサブプロセス呼び出し）
casa get living_aircon 0x80

# プロパティの書き込み
casa set living_aircon 0x80 0x30

# 電源 ON / OFF のショートカット
casa on living_aircon
casa off living_aircon

# 色温度変更のショートカット（Matter のみ。--kelvin / --mireds は排他）
casa color-temp living_light --kelvin 2700
casa color-temp living_light --mireds 370 --transition 30

# プロパティマップ（introspection）
casa describe living_aircon

# 一覧に各デバイスのプロパティマップを含める（その場で取得。永続キャッシュなし）
casa list --describe

# 設定ファイルの妥当性チェック（実機は呼ばない）。アダプタ未実装プロトコルを警告する
casa validate
```

`get` / `set` の 2 つ目の引数 `<property>` の解釈はプロトコル依存:
ECHONET Lite なら EPC（例 `0x80`）、Matter なら `endpoint/cluster/attribute`
（例 `1/onoff/on-off`）。

`casa validate` は設定を読んで妥当性を JSON で報告する（version・必須フィールド・
未知プロトコルは読み込み時点で検証済み）。加えて、設定としては妥当だが実行時に
`protocol_unsupported`（exit 14）になるアダプタ未実装プロトコルを `warnings` に出す:

```json
{
  "timestamp": "2026-06-02T12:34:56+09:00",
  "config": "/home/you/.config/casa/devices.toml",
  "version": 1,
  "device_count": 2,
  "protocols": { "echonet": 1, "switchbot": 1 },
  "warnings": [
    { "kind": "no_adapter", "device": "entry_lock", "protocol": "switchbot",
      "detail": "protocol \"switchbot\" has no adapter yet; get/set/on/off will fail at runtime" }
  ],
  "valid": true
}
```

stdout には純粋な構造化 JSON のみを出す。`timestamp`（ISO 8601）を必ず含む。

```json
{
  "timestamp": "2026-06-02T12:34:56+09:00",
  "devices": [
    { "name": "living_aircon", "protocol": "echonet", "ip": "192.0.2.10", "eoj": "0x013001" }
  ]
}
```

`get` / `set` は子 CLI の出力を casa のスキーマに再整形して出す:

```json
{
  "timestamp": "2026-06-02T12:34:56+09:00",
  "device": "living_aircon",
  "protocol": "echonet",
  "value": { "power": "on" }
}
```

診断ログは stderr に構造化（JSON）で出る。レベルは `RUST_LOG` で制御する。

```bash
RUST_LOG=debug casa list
```

## 設定ファイル

既定パス: `$XDG_CONFIG_HOME/casa/devices.toml`（既定 `~/.config/casa/devices.toml`）。
`--config <path>` または環境変数 `CASA_CONFIG` で上書きできる。

設定ファイル自体はこのリポジトリでは管理しない。利用者が別リポジトリで持ち、
`~/.config/casa/` に配置またはシンボリックリンクする。

サンプル（ダミー値のみ）: [examples/devices.toml](examples/devices.toml)

```toml
version = 1

[devices.living_aircon]
protocol = "echonet"
ip = "192.0.2.10"
eoj = "0x013001"

[devices.entry_lock]
protocol = "switchbot"
device_id = "DUMMY-XX-XX"
```

## 子 CLI（兄弟 CLI）

casa はプロトコル固有 CLI が `PATH` 上に存在することを前提とする。

| プロトコル | CLI | 想定最低バージョン | 状態 |
|---|---|---|---|
| ECHONET Lite | `enl` | 0.1.0（`get` / `set` が stdout に JSON を出すこと） | 対応済み |
| Matter | `mat` | `read` / `write` / `invoke` / `on` / `off` / `color-temp` / `describe` が stdout に JSON を出すこと | 対応済み |
| SwitchBot | `switchbot` | — | 未対応（Phase 4） |

casa が呼ぶ enl のインターフェース（enl の出荷に合わせて追従する）:

```
enl get --ip <ip> --eoj <eoj> --epc <epc>
enl set --ip <ip> --eoj <eoj> --epc <epc> --value <value>
enl describe --ip <ip> --eoj <eoj>
```

casa が呼ぶ mat のインターフェース（Matter は (node_id, endpoint, cluster, attribute) でアドレスする）:

```
# casa get <name> <property>  property = endpoint/cluster/attribute（chip-tool 表記）
mat read <node_id> <endpoint> <cluster> <attribute>
# casa set <name> <property> <value>
mat write <node_id> <endpoint> <cluster> <attribute> <value>
mat describe <node_id>
# casa color-temp <name> --kelvin <k> | --mireds <m> [--transition <t>]
mat color-temp --node <node_id> [--endpoint <ep>] --kelvin <k> | --mireds <m> [--transition <t>]
```

Matter デバイスは設定で `node_id` を必須、`endpoint`（on/off ショートカット用、既定は mat 側の 1）を任意で持つ:

```toml
[devices.living_light]
protocol = "matter"
node_id = "1234"          # commission 済みノードの識別子

[devices.power_strip_outlet2]
protocol = "matter"
node_id = "5678"
endpoint = 2              # on/off が対象とするエンドポイント
```

`get` / `set` の `<property>` は `endpoint/cluster/attribute` 形式（例: `casa get living_light 1/onoff/on-off`、`casa set living_light 1/levelcontrol/current-level 128`）。casa はこのセレクタを解釈せず `/` で分解して mat に渡すだけで、妥当性は mat（chip-tool）側が検証する。

### `on` / `off` の対応状況とマッピング先

ショートカットのマッピングはプロトコルロジックではなく UX として casa 内に
ハードコードしている。

| プロトコル | `on` | `off` | マッピング先 |
|---|---|---|---|
| ECHONET Lite | ○ | ○ | EPC `0x80` に `0x30`（ON）/ `0x31`（OFF）を set |
| Matter | ○ | ○ | OnOff クラスタの On / Off コマンドを invoke（`mat on`/`off`、エンドポイントは設定の `endpoint`） |
| SwitchBot | × | × | 未対応（Phase 4 でアダプタ追加時に対応） |

`describe` も同様: ECHONET Lite は `enl describe`（プロパティマップ）、Matter は `mat describe`
（ノードの endpoint / cluster introspection）、SwitchBot は未対応
（`casa describe` は exit 14、`casa list --describe` では `properties: null`）。

`color-temp` は Matter のみ対応: 色温度変更は属性 write ではなく ColorControl コマンドの
invoke なので、`mat color-temp` に委譲する（エンドポイントは設定の `endpoint`）。
`--kelvin` / `--mireds` はどちらか一方が必須（排他は clap が検証、exit 2）。
範囲外の値は mat / デバイス側が clamp し、casa は事前検証しない。
ECHONET Lite / SwitchBot は未対応（exit 14 `protocol_unsupported`）。

バイナリの解決は `PATH` が既定。以下で上書きできる（環境変数が優先）:

- 環境変数: `CASA_ENL_BIN=/path/to/enl`
- 設定ファイル:

  ```toml
  [binaries]
  enl = "/path/to/enl"
  ```

子 CLI の stderr は呑み込まず、`RUST_LOG=debug` で casa の stderr に転送される。

## exit code

| code | 意味 |
|---|---|
| 0 | 成功 |
| 2 | CLI 引数エラー（clap 既定） |
| 10 | 設定ファイル無し / パース失敗 |
| 11 | 名前が設定ファイルに無い |
| 12 | 子 CLI バイナリが見つからない / 実行不可 |
| 13 | 子 CLI の stdout が JSON としてパースできない |
| 14 | そのプロトコルでは未対応の操作 |
| その他 | 子 CLI の exit code をそのまま伝播 |

子 CLI 由来のエラーは元の exit code を保つ（例: enl がタイムアウトの `3` で終了したら
casa も `3` で終了する）ので、呼び出し側が「タイムアウトかリジェクトか」等を区別できる。

## stderr エラー形式

casa 自体のエラーは stderr に 1 行 JSON で出る:

```json
{"error": {"kind": "config_missing", "detail": "config file not found: ..."}}
```

`kind` の値は安定しており、以下がすべて:

| kind | 意味 | exit code |
|---|---|---|
| `config_missing` | 設定ファイルが存在しない | 10 |
| `config_parse` | 設定ファイルのパース / バリデーション失敗 | 10 |
| `name_not_found` | 名前が設定ファイルに無い | 11 |
| `child_not_found` | 子 CLI バイナリが見つからない / 実行不可 | 12 |
| `child_failed` | 子 CLI が非ゼロで終了（コードを伝播） | 子 CLI のコード |
| `child_invalid_output` | 子 CLI の stdout が JSON でない | 13 |
| `protocol_unsupported` | そのプロトコルでは未対応の操作 | 14 |

## casad（常駐レイヤ: 自動化ルール）

「A が起きたら B する」「時刻になったら C する」といった自動化は **casa 本体には持たせない**
（casa はステートレスを維持する）。代わりに同じワークスペースの別バイナリ `casad` が担う。
詳細は [CLAUDE.md](CLAUDE.md) の「常駐・状態」節を参照。

- **casa** = ステートレスな実行役（CLI）。
- **casad** = 常駐してルールを評価し、発火時に **casa を子プロセスとして呼ぶ**。設定ロードと
  名前解決は `casa-core` を共有（link）し、実機アクションは casa に委譲する（ハイブリッド）。

ルールは TOML で書く（書き手は LLM / UI を想定。サンプル: [examples/rules.toml](examples/rules.toml)）:

```toml
version = 1

# イベントトリガ: living_aircon の電源(EPC 0x80)が ON(0x30) になったら寝室灯を点ける
[[rules]]
name = "エアコン起動で寝室灯ON"
when = { device = "living_aircon", epc = "0x80", equals = "0x30" }
then = { action = "on", device = "bedroom_light" }

# 時刻トリガ: 毎日 22:00 に寝室灯を消す
[[rules]]
name = "22時に寝室消灯"
when = { at = "22:00" }
then = { action = "off", device = "bedroom_light" }
```

```bash
# ルールをパース・検証し、casad の解釈を JSON で返す
casad check rules.toml

# 常駐起動（時刻スケジューラ + enl listen のイベントリスナを並行に回す）
casad run rules.toml

# デバッグ: 時刻トリガを 1 回だけ評価（cron 毎分起動の委譲もこの形）
casad run rules.toml --once --now 22:00

# デバッグ: enl listen を 1 回だけ回してイベントトリガを評価
casad run rules.toml --listen-once
```

イベントトリガは enl の `listen`（INF 通知の待受）をループで回して実現する。enl のバイナリ
解決・stderr 転送は casa と同じ規約（`CASA_ENL_BIN` / `[binaries]` / `PATH`）に従う。

## 開発

ワークスペース構成（`crates/`）:

| crate | 種別 | 役割 |
|---|---|---|
| `casa-core` | lib | 設定ロード・名前解決・アダプタ・子 CLI ランナー（casa と casad が共有） |
| `casa` | bin | ステートレス CLI |
| `casad` | bin | 常駐レイヤ（ルール DSL エンジン） |

```bash
cargo build
cargo test
cargo clippy --workspace -- -D warnings
RUST_LOG=debug cargo run -p casa -- list --config examples/devices.toml
RUST_LOG=debug cargo run -p casad -- check examples/rules.toml --config examples/devices.toml
```

### 新しいプロトコルの追加

プロトコル固有の知識は `crates/casa-core/src/adapter/` に閉じている。新プロトコルの追加は次の
3 点だけで、サブコマンドハンドラ（`crates/casa/src/main.rs` / `crates/casa-core/src/ops.rs`）は
変更しない:

1. `crates/casa-core/src/config.rs` の `Device` enum に variant を追加する。
2. `crates/casa-core/src/adapter/` にその variant の子 CLI 引数を組むアダプタを実装し、
   `adapter_for` に 1 行足す。
3. アダプタのユニットテストを追加する。

CI では実 enl を使わない。統合テストは `crates/casa/tests/fixtures/`（casa）と
`crates/casad/tests/fixtures/`（casad: casa / enl の代役スタブ）のダミーで行う。

### 実機相手の手動 E2E テスト（CI には載せない）

実 enl と実機がある環境で以下を確認する:

```bash
# 1. 実機を指す設定ファイルを用意し（別リポジトリ管理）、デバイス一覧が出ること
casa list

# 2. 動作状態（EPC 0x80）が読めること
casa get living_aircon 0x80

# 3. 書き込みが反映されること（0x30 = ON）と、再読み取りで確認
casa set living_aircon 0x80 0x30
casa get living_aircon 0x80

# 4. 機器の電源を切る等で到達不能にし、enl のタイムアウト exit code が
#    そのまま casa から返ること（echo $? で確認）
casa get living_aircon 0x80; echo $?
```
