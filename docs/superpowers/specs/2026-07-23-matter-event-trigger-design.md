# casad Matter イベントトリガ対応 — 設計

日付: 2026-07-23
状態: 承認済み

## 目的

casad のイベントトリガを Matter に拡張し、`mat listen`(matd の常駐 Subscribe への薄いクライアント)経由でデバイスの状変を受けてルールを発火できるようにする。

最初のユースケース: 書斎の人感センサー `study_motion`(Matter node 16, occupancysensing)の occupancy が 0 になったら `desk_tape_light`(node 6)を消灯、1 になったら点灯。

## 背景と方式選定

- casad のイベントソースは現状 `enl listen`(ECHONET INF)のみ。
- `mat listen` は mat 1.0.0 で実装済み。購読状態は matd が常駐保持し、`mat listen` は unix socket で接続して 1 行 1 JSON を stdout に中継するだけの薄いクライアント(matd 不在 = exit 13)。
- **方式 A(採用)**: enl と対称の one-shot ループ。casad が `mat listen --count 1 --timeout-ms 0` を繰り返し起動する。casad の結合は mat の stdout JSON スキーマのみで、casa ファミリーの「プロトコルは子 CLI に委譲」原則を維持する。
- 方式 B(不採用): casad が matd の socket に直結。プロセス起動ゼロだが、matd のワイヤプロトコルに casad が直接結合する例外を作る。ローカル socket + 人感頻度では実益がない。

### 検証済みの前提(mat / matd 実装確認)

- matd はイベントを tokio broadcast で配るため、listen クライアントは**接続以降**のイベントだけ受け取る。新規接続への過去分再配達はなく、one-shot ループが busy loop になることはない。
- matd は自身の(再)購読時に現在状態の全量を `priming: true` イベントとして接続中クライアントへ再配達する。casad はこれを捨てないと再購読のたびに現在値で誤発火する。
- イベント行のスキーマ: `{timestamp, node_id(数値), endpoint(数値), cluster(chip-tool 名 or 数値), attribute(chip-tool 名 or 数値), value(JSON 値), priming(bool)}`。occupancy の value は数値(0/1)。

## DSL / 設定

```toml
# devices.toml(別リポジトリ管理・jarvis 側)に追加
[devices.study_motion]
protocol = "matter"
node_id = "16"

# rules.toml に追加
[[rules]]
name = "書斎 人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }

[[rules]]
name = "書斎 人感ONで点灯"
when = { device = "study_motion", attribute = "occupancy", equals = 1 }
then = { action = "on", device = "desk_tape_light" }
```

- `Trigger`(untagged enum)に Matter イベント variant を追加する。キーで判別: `epc` あり = ECHONET、`attribute` あり = Matter。`at` あり = 時刻(既存)。
- `equals` は TOML 値(整数 / bool / 文字列)。matd イベントの JSON `value` との等値比較。ECHONET の hex 正規化(`norm_hex`)は使わない別系統。
- ロード時検証(`RuleFile::validate`): Matter トリガの `when.device` は config 上 `protocol = "matter"` のデバイスであること。違反は発火前に弾く(既存の「不正ルールは起動前に弾く」方針の延長)。`then.device` は従来どおり任意プロトコル / グループ可。

## casad コンポーネント

### `mat.rs`(新規、enl.rs と対称)

- `listen_once(bin) -> Result<Vec<Event>, CasaError>`: `mat listen --count 1 --timeout-ms 0` を起動し、stdout の 1 行 JSON を `Event { node_id: u64, endpoint: u64, cluster: Value, attribute: Value, value: Value, priming: bool }` にパースして返す。
- stderr は debug ログへ転送(enl と同じ)。非 0 終了は `ChildFailed`、JSON 不正は `ChildInvalidOutput`。
- バイナリ解決は enl と同じ規約: `CASA_MAT_BIN` / `[binaries]` / PATH(casa-core の runner 規約を流用)。

### `rules.rs`

- `Trigger::MatterEvent { device, attribute, equals }` を追加(`equals` は toml 値 → 比較時に JSON 値へ変換)。
- validate に protocol 一致チェックを追加。

### `engine.rs`

- `matter_event_matches(rule, config, event)`:
  1. `priming == true` は無条件で不一致(誤発火防止)。
  2. device を config で解決し `protocol = "matter"` でなければ不一致。
  3. node_id: config の文字列(例 "16")を数値へ正規化して比較。
  4. config に `endpoint` があれば数値比較、なければ endpoint は不問。
  5. attribute 名: case-insensitive 比較(matd は chip-tool の小文字名、未知 ID は数値のまま流す)。
  6. `event.value == equals`(TOML→JSON 変換後の等値)。
- 常駐時: Matter イベントルールが 1 件以上あるときだけ 3 本目のスレッドで mat 用 event_loop を起動。既存 Dispatcher に積む(同一デバイス FIFO・異デバイス並列は既存のまま)。
- 失敗時(matd 不在 exit 13 等)は enl と同じ warn + 5 秒バックオフ。
- `--listen-once` 相当の同期経路も enl と対称に用意する(デバッグ用)。

## 既知の割り切り

- one-shot ループの再接続の隙間(ミリ秒オーダー)のイベントは取りこぼしうる。enl listen ループと同じ性質。人感センサーは on/off が交互に来るため、落としても次の遷移で自己回復する。
- cluster はルール DSL に含めない(device + attribute + equals で十分)。同一デバイスで属性名が衝突するクラスタが現れたら拡張する。

## テスト

- 単体(rules.rs): Matter トリガのパース、equals の型(整数 / bool)、protocol 不一致の validate エラー。
- 単体(engine.rs): matter_event_matches — 一致 / node_id 違い / attribute 違い / 値違い / priming スキップ / endpoint 指定時の一致・不一致 / attribute 大文字小文字。
- 統合(tests/): 固定 JSONL を吐くダミー `mat` スクリプト(既存 `enl_listen.sh` fixture と同型)で listen → 発火を通す。実 matd は CI に不要。

## デプロイ(実装完了後)

1. cross build(aarch64)→ jarvis へ配布(despliegue の手順)。
2. jarvis の `~/.config/casa/devices.toml` に `study_motion` を追加(別リポジトリ管理)。
3. `~/.config/casa/rules.toml` に上記 2 ルールを追加。
4. casad.service 再起動。
5. 実センサーで E2E 確認(在室 → 点灯、退室 → 消灯)。
