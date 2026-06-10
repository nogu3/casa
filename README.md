# casa

スマートホーム横断クライアント。プロトコル固有 CLI（`enl` 等）をサブプロセスで呼び、
人間に優しい名前のマッピングと統一された CLI UX を提供する。

casa はプロトコルを直接喋らない。バイト列もソケットも持たず、すべて兄弟 CLI に委譲する。
設計の全体像は [CLAUDE.md](CLAUDE.md) を参照。

## 使い方

```bash
# 設定済みデバイスの一覧
casa list
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

## exit code

| code | 意味 |
|---|---|
| 0 | 成功 |
| 2 | CLI 引数エラー（clap 既定） |
| 10 | 設定ファイル無し / パース失敗 |
| 11 | 名前が設定ファイルに無い |
| その他 | 子 CLI の exit code をそのまま伝播 |

## stderr エラー形式

casa 自体のエラーは stderr に 1 行 JSON で出る:

```json
{"error": {"kind": "config_missing", "detail": "config file not found: ..."}}
```

| kind | 意味 | exit code |
|---|---|---|
| `config_missing` | 設定ファイルが存在しない | 10 |
| `config_parse` | 設定ファイルのパース / バリデーション失敗 | 10 |
| `name_not_found` | 名前が設定ファイルに無い | 11 |

## 開発

```bash
cargo build
cargo test
cargo clippy -- -D warnings
RUST_LOG=debug cargo run -- list --config examples/devices.toml
```
