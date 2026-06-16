//! casa の共有ロジック。casa バイナリ（CLI）と casad（常駐）が link して使う。
//!
//! 設計境界: ここに置くのは「ステートレスな純ロジック」だけ —— 設定ロード・名前解決・
//! プロトコルアダプタ・子 CLI ランナー。常駐・スケジューラ・状態保持はここに持ち込まない
//! （それらは casad の責務）。

pub mod adapter;
pub mod config;
pub mod error;
pub mod ops;
pub mod output;
pub mod runner;
