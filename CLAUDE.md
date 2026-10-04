# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 概要

rireki は Claude Code / Codex のセッションログ (JSONL) を読み込み、ツール呼び出しやメッセージをタイムライン／カレンダー表示する macOS 向けデスクトップアプリ。Rust (edition 2024) + GPUI 製。

## コマンド

```bash
cargo build                 # ビルド
cargo run                   # GUI 起動
cargo run -- --stats        # GUI を起動せず、索引 DB の読み込みと差分取り込みを行い、件数と所要時間を表示
cargo clippy                # lint
cargo test                  # テスト（現状テストは無い）
```

`.cargo/config.toml` で linker / CC を `/usr/bin/cc` に固定している（Nix の cc が libiconv を見つけられないため）。変更しないこと。

## アーキテクチャ

データの流れ: 起動時に `index::Indexer::load_stable`（索引 DB）→ `model::Store::replace` → 描画。その後 `Indexer::plan` / `process` で未索引・更新ファイルを取り込み、3 秒ごとにリアルタイムファイルを再読込する。

- **index.rs** — ファイルを「安定」（最終更新から `LIVE_SECS` 以上経過）と「リアルタイム」に分ける。安定ファイルはパース結果を DB に保存し、指紋（mtime + サイズ）が変わるまで読み直さない。リアルタイムファイルは DB に書かずメモリ上で毎回パースし、更新が止まると安定側へ移る。
- **db.rs** — 索引 DB (`~/Library/Caches/rireki/index.db`, SQLite/rusqlite)。`files` / `items` / `lanes` / `events` / `texts`。起動時は `events` だけを ID 範囲で分割して並列に読み、大きな input / output (`texts`) は詳細表示のときに `Event::row` で引く。`texts` は lz4 圧縮して別ファイル `texts.db` に置く（同じファイルだと `events` のページが散らばり起動が遅くなる）。書き込みは `Indexer` の専用スレッドで行い、次のチャンクのパースと重ねる。パース結果やスキーマを変えたら `VERSION` を上げる（DB が作り直される）。
- **loader.rs** — 読み込み元は `~/.claude/projects/*/*.jsonl` と `$CODEX_HOME/sessions`（既定 `~/.codex/sessions`、再帰探索）。ファイルは mtime の新しい順に `Job` 化される。
  - Claude のサブエージェントは 2 形式に対応: 新形式 `<session>/subagents/agent-*.jsonl` + `.meta.json`（`toolUseId` を `Lane::spawned_by` に入れる）と、旧形式のプロジェクト直下 `agent-*.jsonl`（`Loaded::Child` として親セッション ID 付きで返す）。
  - ツール呼び出しと結果は `Pending`（call_id → イベント index）で突き合わせ、結果のタイムスタンプで `end` を確定する。テキストは `MAX_TEXT` で切り詰める。
- **jsmock.rs** — Codex のコードモード (`exec`) の JS を QuickJS (rquickjs) で実行し、`tools` を Proxy にしたモックで呼び出しを記録する。読み込み時は静的解析 (`js_tool_calls`) で名前と要約だけを決め、input には JS を `JS_MARK` 付きで保存する。詳細表示時に `loader::exec_detail` で整形し、結果を `jscache.db`（キーは JS 本文のハッシュ）にキャッシュする。モックの挙動を変えたら `ENGINE_VERSION` を上げる。
- **model.rs** — `Session` は複数の `Lane`（main + サブエージェント）を持ち、`Lane` は時刻順の `Event` 列。`Store` はレーンを読み込み元ファイル (`Lane::origin`) 単位で差し替え、親より先に届いた子レーンを `orphans` に保持して親セッション追加時に合流させる。描画用の派生データ（`cum_end`, `parents`, `spawn_t`）は `Session::finalize` で事前計算し、描画中に全イベントを走査しない。時刻はすべて Unix 秒の `f64`。
- **layout.rs** — 表示スケール（`spp` = 1px あたり秒数）と `Granularity`（自動／同一ツール／ターン／コール）に応じて、レーンのイベントを `Block`（events の index 範囲）に集約する。表示範囲外のブロックは捨てる。
- **app.rs** — GPUI の `Rireki` ビュー。`Mode`（Day / Week / Timeline）の切り替え、cwd ごとのグループ見出しと折りたたみ、ホイールでのズーム・スクロール、ドラッグでのパン、選択ブロックの詳細表示を担う。
- **theme.rs** — 配色と、ツール名 → 色のマッピング（Claude と Codex のツール名を同じカテゴリにまとめる）。新しいツール名を色分けする場合は `tool_color` と `LEGEND` を合わせて更新する。

コメントや UI 文言は日本語で書かれている。
