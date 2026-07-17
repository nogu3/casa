# casad 発火の非同期化 — デバイス別ワーカー設計

- 日付: 2026-07-17
- 状態: 承認済み（ブレインストーミング完了）
- 対象: `crates/casad`（casa / casa-core は無変更）

## 背景と動機

現行の常駐エンジンは `event_loop` が `enl listen → 突合 → fire_all（直列・同期）→ 再 listen`
で回っており、アクション実行中（matd warm 経由で実測 ~0.7s、cold ~1.3s、matd 不達時の
フォールバックでは数秒〜）は `enl listen` が止まる。この間の INF 通知は取りこぼす。
また `time_loop`（毎分 tick）と `event_loop` が独立に `fire_all` を呼ぶため、時刻ルールと
イベントルールが同一デバイスへ同時に casa を起動し得る。

ゴールは 2 つ:

1. **listen の取りこぼし窓を消す** — アクション実行を listen ループから切り離す。
2. **異デバイス間の並列実行** — 複数ルール同時発火時の総レイテンシを max(各アクション) に。

同時に守る不変条件:

- **同一デバイス宛アクションの FIFO 順序**。ON→OFF が逆順で実機に届く事故を構造的に排除する。

## 決定事項

| 論点 | 決定 |
|---|---|
| 並行モデル | デバイス別ワーカー（同一デバイス直列 / デバイス間並列） |
| time_loop | 同じディスパッチャに統一（時刻×イベントの同一デバイス競合も解消） |
| `--once` | 従来どおり同期 `fire_all`（cron の「終了 = 全アクション完了」を維持） |
| キュー | `std::sync::mpsc` 無制限。溢れる現実性がなく、下流障害時は滞留 + warn ログで気づく方を選ぶ |
| 依存追加 | なし（std のみ） |

## アーキテクチャ

```
event_loop ──┐  dispatch(&Rule)
             ├──▶ Dispatcher ─┬─▶ worker[device A] ─▶ fire() ─▶ casa
 time_loop ──┘                ├─▶ worker[device B] ─▶ fire() ─▶ casa
                              └─▶ ...（デバイス別 FIFO・デバイス間並列）
```

### 新モジュール `crates/casad/src/dispatch.rs`

- 常駐モード起動時に rules.toml の `then.device` の distinct 集合を取り、デバイスごとに
  `mpsc::channel::<&Rule>()` + ワーカースレッド 1 本を既存の `thread::scope` 内に張る。
  scoped thread なので `&Rule` / `Option<&Path>` を Arc なしで借用したまま送れる。
- `Dispatcher` は `HashMap<デバイス名, Sender<&Rule>>`。`dispatch(rule)` は `then` の
  デバイス名でチャネルを引いて送信し即戻る。event_loop / time_loop へはそれぞれ clone を
  渡す（`Sender: Sync` の MSRV（1.72）に依存しない）。
- ワーカーは受信ループで既存の `fire()` を呼ぶだけ。成功 / 非ゼロ exit / spawn 失敗の
  warn ログは現行 `fire_all` と同一形式をワーカー側に移す。
- `rules::Then` に `device()` アクセサを追加（on / off / invoke すべて device を持つ）。

### 実行モードごとの経路

- **常駐（`casad run`）**: event_loop は `listen → 突合 → dispatch → 即再 listen`。
  time_loop は `due_time_rules → dispatch → 次の分境界まで sleep`。listen の取りこぼし窓は
  enl 再 spawn の数 ms のみになる。
- **`--once`**: 同期 `fire_all` を維持。`fire_all` は --once 専用として残す。
- 発火件数の戻り値: 常駐経路は「キューに積んだ件数」を返し、ログ文言も合わせる。

## 順序とエラーの扱い

- 同一デバイス宛は「rules.toml 記載順 → イベント到着順」の FIFO。
- ワーカーはアクション失敗で死なない（warn して次ジョブへ）。チャネル切断での終了は
  scope 終了時のみで、常駐では到達しない。
- `then.device` が devices.toml に無い場合は従来どおり casa が exit 11 → warn。
  `casad check` の事前検証は現状のまま。
- 既知の限界（許容）: group 名とその構成メンバーを別ルールが同時に叩く場合、ワーカーが
  別なので相対順序は保証しない。casad 経由の Matter アクションは全て matd に集約される
  （2026-07-17 の `MAT_MATD_SOCKET` drop-in で保証）ため、matd 側で直列化される。

## テスト

- **dispatch 単体**（`CASA_BIN` にフェイクバイナリを差す既存方式を流用）:
  - 同一デバイス 2 ジョブが送信順に完走する（フェイクが引数と時刻をファイルに追記）。
  - `dispatch()` がアクション完了を待たずに戻る（slow フェイクで所要時間を計測）。
  - 異デバイスが並行実行される（device A に sleep フェイク、B の完了が A の完了に
    先行することを確認。タイミング依存のため余裕を持ったマージンにする）。
- **distinct デバイス抽出**の単体テスト。
- 回帰: 既存の `tests/{events,exec,run}.rs` と `--once` 経路は無変更で通ること。

## 変更範囲

- `crates/casad/src/dispatch.rs` — 新規
- `crates/casad/src/engine.rs` — event_loop / time_loop / run の配線
- `crates/casad/src/rules.rs` — `Then::device()` アクセサ
- casa / casa-core — 無変更

## スコープ外

- rules.toml のホットリロード（現状も無い。ワーカー集合は起動時固定でよい）
- キューの coalescing / dedup（ON 連打はべき等、ON→OFF→ON も FIFO なら最終状態が正しい）
- graceful shutdown（casad は現状 `-> !` のループ構成。本設計で変えない）
