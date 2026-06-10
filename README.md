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
| SwitchBot | `switchbot` | — | 未対応（Phase 4） |
| Matter | 未定 | — | 未対応 |

casa が呼ぶ enl のインターフェース（enl の出荷に合わせて追従する）:

```
enl get --ip <ip> --eoj <eoj> --epc <epc>
enl set --ip <ip> --eoj <eoj> --epc <epc> --value <value>
```

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

## 開発

```bash
cargo build
cargo test
cargo clippy -- -D warnings
RUST_LOG=debug cargo run -- list --config examples/devices.toml
```

CI では実 enl を使わない。統合テストは `tests/fixtures/` のダミー enl
（固定 JSON を吐くシェルスクリプト）で行う。

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
