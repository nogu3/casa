# casad ルールの複数アクション(`then` 配列)対応 — 設計

日付: 2026-07-23
状態: 承認済み

## 目的

1 つのルールの `when` に対して複数の `then` を書けるようにする。

現状 `Rule.then` は単一の `Then` で、同じトリガから 2 デバイスを操作するにはルールを 2 本書くしかない。実運用(jarvis)では書斎の人感センサー `study_motion` に対し `desk_tape_light` と `desk_light` を連動させるため、ON/OFF あわせて 4 本のルールが `when` を重複して持っている。

group(`[groups.x] members = [...]`)でも同一アクションの複数デバイス配信はできるが、**デバイスごとに違うアクション**は表現できない。想定される具体的な要求は「点灯し、かつ色温度を変える」であり、これが group では書けないことが本対応の動機である。

## 背景

`crates/casad/src/rules.rs` の冒頭コメントは複数アクションを明示的に後回しにしている:

> 中身は `when`(トリガ)→ `then`(アクション)の素朴な対応。複数条件・遅延・複数アクションなどの表現力拡張は後段で必要になったら足す。

本設計はそのうち「複数アクション」のみを実装する。複数条件と遅延は対象外。

### 守るべき既存の不変条件

`docs/superpowers/specs/2026-07-17-async-rule-firing-design.md` が定めた不変条件:

> **同一デバイス宛アクションの FIFO 順序**。ON→OFF が逆順で実機に届く事故を構造的に排除する。

これは `crates/casad/src/dispatch.rs` がワーカーを `rule.then.device()` をキーに張ることで実現されている。「1 ルール = 1 デバイス」を前提にしているため、複数 `then` が別デバイスを指すとこの対応関係が崩れる。本設計の中心はここをどう解くかである。

## 方式選定

### 採用: アクション単位でファンアウト

ディスパッチの単位を「ルール」から「(ルール, アクション)」に変える。N 個の `then` はそれぞれの対象デバイスのワーカーへ個別に積まれる。

```
then = [
  { action = "on",     device = "desk_light" },
  { action = "invoke", device = "desk_light", command = "color-temp", args = ["--kelvin","2700"] },
  { action = "on",     device = "desk_tape_light" },
]

 dispatch
  ├─ worker[desk_light]      ← on → color-temp  (記載順・直列)
  └─ worker[desk_tape_light] ← on               (並列)
```

- 同一デバイス宛の then は同じチャネルへ記載順に入るため順序が保証される
- 別デバイス宛は別ワーカーなので並列に走る
- ワーカーのキーは従来どおりデバイス名のままで、per-device FIFO 不変条件が維持される

### 不採用: ルール単位で 1 ワーカー直列

ルール全体を 1 つのワーカーに載せ、`then` を上から順に直列実行する案。全 `then` の順序は厳密に保証されるが、

- ワーカーのキーをどのデバイスにするかが決まらない
- 同じデバイスを触る別ルールと別ワーカーに分かれ、FIFO 保証が壊れる
  (例: ルール A の `then` が `desk_light`、ルール B の `then` が `[x, desk_light]` のとき、`desk_light` 宛が 2 本のワーカーから競合する)
- 別デバイス宛が並列に走らず遅くなる

順序保証の強化と引き換えに既存の不変条件を壊すため採らない。

## DSL / スキーマ

`then` は単一テーブルと配列の両方を受け付ける。

```toml
# 既存記法(そのまま動く)
[[rules]]
name = "書斎 人感OFFで消灯"
when = { device = "study_motion", attribute = "occupancy", equals = 0 }
then = { action = "off", device = "desk_tape_light" }

# 新記法
[[rules]]
name = "書斎 人感ONで点灯"
when = { device = "study_motion", attribute = "occupancy", equals = 1 }
then = [
  { action = "on",     device = "desk_light" },
  { action = "invoke", device = "desk_light", command = "color-temp", args = ["--kelvin", "2700"] },
  { action = "on",     device = "desk_tape_light" },
]
```

両記法を受け付けるのは、jarvis で稼働中の rules.toml と既存テストをそのまま動かすため。`serde(untagged)` で吸収でき、コストはほぼゼロ。

`rules.toml` の `version` は **1 のまま**据え置く。新記法は旧記法の上位集合であり、旧 casad が新ファイルを読めないことは version では表現できない(旧 casad は version 1 を受理してからパースに失敗する)ため、version を上げても得るものがない。

### 型

```rust
pub struct Rule {
    pub name: String,
    pub when: Trigger,
    pub then: Thens,
}

/// 単一テーブルと配列の両方を受ける。TOML 上の見た目で種別が決まる(untagged)。
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Thens {
    One(Then),
    Many(Vec<Then>),
}

impl Thens {
    /// 記載順のアクション列。単一形は 1 要素のスライスとして見せる。
    pub fn actions(&self) -> &[Then] {
        match self {
            Thens::One(t) => std::slice::from_ref(t),
            Thens::Many(v) => v,
        }
    }
}
```

`Then` 自体(`On` / `Off` / `Invoke` の各バリアント、`device()`、`casa_args()`)は変更しない。

`Serialize` もラウンドトリップする(単一形は JSON オブジェクト、配列形は JSON 配列)。`casad check` は `RuleFile` をそのまま JSON 出力するため、既存の単一 then ルールの出力は現状と一字一句変わらない。

`Thens::One` は API 上の非対称を持ち込むため、`Rule.then` を直接 match する箇所は残さず、すべて `actions()` を経由する。

## 実行

### ディスパッチ

```rust
/// ワーカーに積む仕事の単位。ルール名はログ用に持ち回る。
#[derive(Clone, Copy)]
pub struct Job<'env> {
    pub rule: &'env Rule,
    pub then: &'env Then,
}
```

- `Dispatcher` のチャネルは `Sender<Job<'env>>` になる
- `dispatch` のキーは `job.then.device()`(従来 `rule.then.device()`)
- `distinct_devices` は `flat_map(|r| r.then.actions())` で全アクションの対象デバイスを集める
- ルール発火は `rule.then.actions()` を**記載順**にキューへ積む。積めた件数を返す

同一デバイス宛の複数アクションが同じチャネルへ記載順に入ることが、宣言順の実行を保証する。

### 失敗時

途中の `then` が失敗しても後続の `then` は実行する。失敗は warn ログに残す。

既存 `engine.rs` の方針(「個々のアクション失敗はループを止めず warn ログに残す(常駐の頑健性)」)と揃える。打ち切り方式は、別デバイス宛が並列に走るモデルでは「以降」の定義自体が曖昧になるため採らない。

### 同期経路の戻り値

`fire` / `run_one` は `&Rule` ではなく `Job`(あるいは `(&Rule, &Then)`)を取る。

`fire_all`(`--once` / `--listen-once` / `--listen-once-mat` の同期経路)の戻り値は「成功したルール数」から「**成功したアクション数**」に変わる。単一 then のルールでは両者が一致するため、既存テストの期待値は変わらない。README の該当記述も合わせる。

### ログ

多 then ルールが 1 行に潰れると journal で追えないため、発火ログに対象デバイスとアクション種別を足す。

```
INFO firing rule rule="書斎 人感ONで点灯" device="desk_light" action="on"
INFO firing rule rule="書斎 人感ONで点灯" device="desk_light" action="invoke"
INFO firing rule rule="書斎 人感ONで点灯" device="desk_tape_light" action="on"
```

失敗ログ(`rule action exited nonzero` / `rule action failed`)にも同じフィールドを足す。どの then が落ちたか判別できないと多 then ルールのデバッグができないため。

`action` の値は `Then` のバリアント名を小文字化したもの(`on` / `off` / `invoke`)。

## 検証

`rules.rs` の `check_target`(現状 `rule.then.device()` を 1 回検査)を全アクションに対して回す。存在しないデバイス/グループを指す `then` が 1 つでもあれば `casad check` と起動時に弾かれる。現行の「発火前に不正なルールを弾く」方針の維持。

`then = []` は検証エラー(`ErrorKind::ConfigParse`)にする。何も起きないルールは書き間違い以外にありえない。エラーメッセージにルール名を含める。

## テスト

既存のテスト構成にそのまま載る。

`crates/casad/src/rules.rs`:
- 配列形の `then` をパースし、`actions()` が記載順に 3 要素返す
- 単一形の `then` をパースし、`actions()` が 1 要素返す(既存テストの維持)
- 同一ファイル内に単一形と配列形が混在してもパースできる
- `then = []` が `ConfigParse` エラーになり、メッセージにルール名が含まれる
- 配列内の 1 要素だけが未定義デバイスを指す場合に検証エラーになる
- 配列形の `Serialize` が配列として往復する

`crates/casad/src/dispatch.rs`(既存の記録クロージャ方式):
- `distinct_devices` が多 then ルールの全対象を重複なく集める
- 多 then ルール 1 件のディスパッチで、各アクションが対象デバイスのワーカーへ振られる
- 同一デバイス宛の 2 アクションが記載順に実行される(先行アクションを sleep させても順序が保たれる)
- 別デバイス宛の 2 アクションが並列に走る(遅いほうを先に積んでも速いほうが先に完走する)

`crates/casad/src/engine.rs`:
- 多 then ルール 1 件の同期発火で、戻り値がアクション数(ルール数ではない)になる
- 途中のアクションが失敗しても後続が実行される

## ドキュメント

- `README.md` の rules.toml 節に配列記法の例を追加。`fire_all` の戻り値の意味が変わる箇所があれば合わせる
- `examples/rules.toml` に複数アクションの例を 1 つ追加
- `CLAUDE.md` の casad 節、現在「`then` supports `on` / `off` / `invoke`」と書いてある箇所に複数アクションを追記
- casa / casad のバージョンを 1.1.0 → 1.2.0

## 対象外

- **複数条件の `when`**。`rules.rs` 冒頭コメントの残りの項目。今回は触らない
- **遅延 / 順序制御の DSL**(`delay = "5s"` など)。同上
- **group の撤去**。同一アクションを複数デバイスへ投げるだけなら group のほうが短いままなので併存させる
- **稼働中 rules.toml の書き換え**。jarvis 上の 4 ルールを 2 ルールへ集約するかは運用判断であり、本対応の完了条件に含めない
