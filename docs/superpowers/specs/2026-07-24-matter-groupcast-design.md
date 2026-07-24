# Matter groupcast 対応（casa の Matter デバイスに group アドレッシングを追加）

- 日付: 2026-07-24
- 対象: casa-core（config / matter adapter）、casa(bin)、jarvis 実機 config
- version bump: workspace `1.2.0` → `1.3.0`（新機能・minor）

## 背景と課題

書斎の照明 `desk_light` を Matter の **wire group（groupcast / multicast）** 化した。
`mat` 側には既にグループが provision 済みで、`~/.config/mat/aliases.toml` に
`[groups] desk_room_lights = 11`（GroupId 11 = desk_light の物理電球群）がある。
groupcast は 1 発の multicast で複数機器を同期制御でき、unicast を機器ごとに
撒くより低遅延・原子的。

一方 casad の書斎 人感トリガ（`~/.config/casa/rules.toml`）は現状 desk_light を
**unicast**（`mat on/off --node 17`）で叩いている。これを groupcast 経由へ切り替えたい。

**課題**: casa は現在 Matter を `--node <node_id>`（unicast）でしか addressing できない。
`mat group invoke` に到達する経路が無い。casa の既存 `[groups]` は client-side fan-out
（メンバーを 1 台ずつ unicast）であり、wire multicast とは別物なので流用不可。

したがって casa に「Matter を group で addressing する」機能を足す必要がある。
casad は無変更（`casa on/off <device>` を spawn するだけ）で、casa バイナリのみ改修・再配布する。

## スコープ

`desk_room_lights`（GroupId 11）は **desk_light の物理電球群のみ**を含む。
`desk_tape_light`（node 6）は従来どおり unicast のまま。よって書斎ルールは
「desk_tape_light は unicast on/off ＋ desk_light の代わりに desk_room_lights へ groupcast on/off」
の 2 アクション構成になる。

本作業は casa 実装 → テスト → docs → jarvis へ deploy → 実機 config 反映 → 検証までを含む。

## 設計方針の決定

`protocol` はあくまで `matter`（子 CLI は `mat`）であり、groupcast は
**addressing mode の違い**にすぎない。別 protocol tag（`matter_group`）は設けない。
既存の `Device::Matter` variant を node / group の両対応に拡張する。

## 1. config（casa-core `config.rs`）

`Device::Matter` を次のとおり変更する:

```rust
Matter {
    /// unicast 宛先。group と排他。
    #[serde(default)]
    node_id: Option<String>,
    /// Matter wire group（groupcast）の宛先。mat の group alias / GroupId を
    /// パススルーする（casa は解釈しない。mat が alias→GroupId 解決）。node_id と排他。
    #[serde(default)]
    group: Option<String>,
    /// OnOff/invoke ショートカットが使うエンドポイント。未指定なら mat 既定(1)。
    #[serde(default)]
    endpoint: Option<u32>,
},
```

- **バリデーション（ロード時）**: Matter デバイスは `node_id` / `group` の
  **ちょうど一方**が必須。両方欠落・両方指定は `config_parse`（exit 10）。
  既存の validate ステップ（groups 整合性検証と同じ場所）に Matter デバイス走査を追加する。
- `protocol()` は node/group いずれも `"matter"` を返す（出力 JSON の `protocol` も `matter`）。
- serde の tagged enum（`#[serde(tag = "protocol")]`）はそのまま。`protocol = "matter"` で
  node_id か group のどちらかを書く。

config 例:

```toml
[devices.desk_room_lights]
protocol = "matter"
group = "desk_room_lights"
```

## 2. adapter（`adapter/matter.rs`）

`address()` を「node か group か」を表す内部 enum に変える:

```rust
enum MatterAddr<'a> {
    Node { node_id: &'a str, endpoint: Option<u32> },
    Group { group: &'a str, endpoint: Option<u32> },
}
```

各 op の分岐（Node 側は従来の挙動を完全維持）:

| op | Node | Group |
|---|---|---|
| `power(on)` | `mat on --node <n> [--endpoint <e>]` | `mat group invoke --group <g> --cluster onoff --command on [--endpoint <e>]` |
| `power(off)` | `mat off --node <n> [--endpoint <e>]` | `mat group invoke --group <g> --cluster onoff --command off [--endpoint <e>]` |
| `invoke(cmd,args)` | `mat <cmd> --node <n> [--endpoint <e>] <args...>` | `mat group <cmd> --group <g> [--endpoint <e>] <args...>` |
| `get` / `set` / `describe` | 従来どおり | `None` → exit 14（groupcast は unacknowledged で読めない） |

Group invoke の具体例:
- `casa invoke desk_room_lights color-temp --kelvin 2700`
  → `mat group color-temp --group desk_room_lights --kelvin 2700`
- `casa invoke desk_room_lights invoke --cluster onoff --command on`
  → `mat group invoke --group desk_room_lights --cluster onoff --command on`

endpoint の注入方針は Node と同じ（config にあれば `--endpoint <e>` を足す。なければ mat 既定 1）。
Group では `--group`/`--endpoint` の前に `group` サブコマンドを 1 語 prepend する点だけが差分。

`mat group invoke` は unacknowledged（"sent" のみ報告）。casa は従来どおり子 CLI の
stdout JSON を casa スキーマ（`timestamp`/`device`/`protocol`/`value`）へ再構成して返す。
`value` に mat の "sent" 応答が入るだけで、on/off の処理経路は node と共通。

## 3. tests

- config パース:
  - group デバイス（`protocol="matter"` + `group`）が正常パース。
  - node_id と group の両指定 → `config_parse`（exit 10）。
  - node_id も group も無い Matter → `config_parse`（exit 10）。
  - 既存の node_id 単独デバイスは従来どおりパース（後方互換）。
- adapter:
  - group `power(on)` / `power(off)` の引数列。
  - group `power` に endpoint 付き。
  - group `invoke`（color-temp shortcut）の引数列。
  - group `invoke`（任意 `invoke --cluster ... --command ...`）の引数列。
  - group `get`/`set`/`describe` が `None`。
  - 既存 node 系テストが全て通る（回帰無し）。

## 4. docs / version

- README: Matter の項に「`node_id` の代わりに `group` を書くと groupcast。
  `get`/`set`/`describe` は非対応（exit 14）、`on`/`off`/`invoke` 対応」を追記。
- CLAUDE.md: Phase 4 の Matter 節へ同旨を 1〜2 行追記。
- workspace version を `1.2.0` → `1.3.0` に bump。

## 5. jarvis 実機反映（casad 無変更、casa バイナリのみ再配布）

1. despliegue skill で casa を cross-build（aarch64）→ jarvis へ scp・差し替え。
2. `~/.config/casa/devices.toml` に追加:
   ```toml
   [devices.desk_room_lights]
   protocol = "matter"
   group = "desk_room_lights"
   ```
3. `~/.config/casa/rules.toml` の書斎 2 ルールで、`device = "desk_light"` の
   2 アクションを `device = "desk_room_lights"` に差し替え（`desk_tape_light` は不変）:
   ```toml
   [[rules]]
   name = "書斎 人感OFFで消灯"
   when = { device = "study_motion", attribute = "occupancy", equals = 0 }
   then = [
     { action = "off", device = "desk_tape_light" },
     { action = "off", device = "desk_room_lights" },
   ]

   [[rules]]
   name = "書斎 人感ONで点灯"
   when = { device = "study_motion", attribute = "occupancy", equals = 1 }
   then = [
     { action = "on", device = "desk_tape_light" },
     { action = "on", device = "desk_room_lights" },
   ]
   ```
4. 実機検証: `casa off desk_room_lights` が `mat group invoke ... --command off` を撃つこと、
   人感 ON/OFF でルールが groupcast を発火することを確認（casad ログの device/action で確認）。

- 注意: `devices.toml` / `rules.toml` は jarvis-iac 管理外の手管理ファイル（IaC 化は
  jarvis-iac issue #3 で別途）。よって ssh 直編集で反映する。反映前に既存内容を退避する。

## 非対象（YAGNI）

- group の `get`/`set`/`describe` を疑似的に読む仕組み（groupcast は本質的に unacknowledged）。
- casa 既存 `[groups]`（fan-out）と Matter wire group の統合・相互変換。
- devices.toml / rules.toml の IaC 化（jarvis-iac issue #3）。

## 影響範囲まとめ

- 変更: `crates/casa-core/src/config.rs`、`crates/casa-core/src/adapter/matter.rs`、
  README、CLAUDE.md、`Cargo.toml`（version）。
- 無変更: casad（`crates/casad`）、他アダプタ（echonet / switchbot）、サブコマンドハンドラ。
- 後方互換: 既存の `node_id` 指定 Matter デバイスは挙動不変。
