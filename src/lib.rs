//! セッション履歴の読み込み・保存・検索・集計。GUI フレームワークに依存しない。
//!
//! ログは [`loader`] が解析し、[`index`] が取り込みと保存を調整する。
//! [`db`] が索引と本文を永続化し、[`model::Store`] は更新を反映して
//! [`model::Snapshot`] を公開する。読み取り側は同じセッションの版を共有する。
//!
//! [`search`]・[`detail`]・[`statistics`] は履歴の問い合わせ、
//! [`layout`]・[`histogram`]・[`cost_timeline`]・[`view_index`] は表示用の計算を担う。
//! [`history`] はフロントエンド共通の読み込み・更新手順、[`catalog`] は一覧の絞り込みを担う。
//! [`mcp`] はスナップショットに対する読み取り API で、取り込みを重複実行しない。
//! GPUI の状態・入力・描画と背景タスクの起動は、バイナリ側の `app` に置く。

pub mod catalog;
pub mod db;
pub mod cost_timeline;
pub mod detail;
pub mod histogram;
pub mod history;
pub mod index;
pub mod jsmock;
pub mod layout;
pub mod loader;
pub mod model;
pub mod mcp;
pub mod patch;
pub mod paths;
pub mod pricing;
pub mod search;
pub mod statistics;
mod text;
pub mod time;
pub mod view_index;
