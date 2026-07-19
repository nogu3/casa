# CI (GitHub Actions) 設計

- 日付: 2026-07-19
- 対象: casa ワークスペース（`crates/casa-core`, `crates/casa`, `crates/casad`）

## 目的

CLAUDE.md の開発コマンド（`cargo build` / `cargo test` / `cargo clippy -- -D warnings`）に
fmt チェックを加えたものを GitHub Actions で自動実行し、main への変更品質を担保する。

## トリガー

- `push`: `main`
- `pull_request`: `main` 向け

## ジョブ構成

単一ジョブ `check`（`ubuntu-latest`, stable toolchain）で以下を順に実行する。
高速な fmt を先頭に置き、崩れを早期に落とす。

1. `actions/checkout`
2. `dtolnay/rust-toolchain@stable`（components: `rustfmt`, `clippy`）
3. `Swatinem/rust-cache`（ビルド／依存キャッシュ）
4. `cargo fmt --all -- --check`
5. `cargo clippy --workspace --all-targets -- -D warnings`
6. `cargo build --workspace`
7. `cargo test --workspace`

## 設計判断

- **単一ジョブ・単一 OS（Ubuntu / x86_64）**。配布先は jarvis(aarch64) だが CI の目的は
  ロジック検証であり、クロスコンパイル検証は含めない（YAGNI）。
- **stable 固定**。MSRV 管理をしていないため toolchain matrix は不要。
- `clippy` は build を兼ねるが、CLAUDE.md に build/clippy 双方が明記されているため両方残す。
  `--all-targets` でテストコードも lint 対象にする。
- サードパーティ action は広く使われる `dtolnay/rust-toolchain` と `Swatinem/rust-cache` を採用。

## 実装時の前提

- 導入時点で `cargo fmt --all -- --check` が 8 ファイルで崩れているため、
  `cargo fmt --all` を適用した整形コミットを同時に入れて CI を緑にする。
- clippy / test は導入時点で通過済み。

## スコープ外

- リリース／バイナリ配布ワークフロー（jarvis 配布は despliegue スキルの手動運用のまま）。
- クロスコンパイル、複数 OS / 複数 Rust バージョンの matrix。
- カバレッジ計測、セキュリティ監査（`cargo audit` 等）。
