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
cargo test                  # テスト
```

`.cargo/config.toml` で linker / CC を `/usr/bin/cc` に固定している（Nix の cc が libiconv を見つけられないため）。変更しないこと。

## アーキテクチャ

データの流れ: 起動時に `index::Indexer::load_stable`（索引 DB）→ `model::Store::replace` → 描画。その後 `Indexer::plan` / `process` で未索引・更新ファイルを取り込み、3 秒ごとにリアルタイムファイルを再読込する。

- **index.rs** — ファイルを「安定」（最終更新から `LIVE_SECS` 以上経過）と「リアルタイム」に分ける。安定ファイルはパース結果を DB に保存し、指紋（mtime + サイズ）が変わるまで読み直さない。リアルタイムファイルは DB に書かずメモリ上で毎回パースし、更新が止まると安定側へ移る。
- **db.rs** — 索引 DB (`~/Library/Caches/rireki/index.db`, SQLite/rusqlite)。`files` / `items` / `lanes` / `events` / `texts`。起動時は `events` だけを ID 範囲で分割して並列に読み、大きな input / output (`texts`) は詳細表示のときに `Event::row` で引く。`texts` は lz4 圧縮して別ファイル `texts.db` に置く（同じファイルだと `events` のページが散らばり起動が遅くなる）。書き込みは `Indexer` の専用スレッドで行い、次のチャンクのパースと重ねる。パース結果やスキーマを変えたら `VERSION` を上げる（DB が作り直される）。
- **loader.rs** — 読み込み元は `~/.claude/projects/*/*.jsonl` と `$CODEX_HOME/sessions`（既定 `~/.codex/sessions`、再帰探索）。ファイルは mtime の新しい順に `Job` 化される。
  - Claude のサブエージェントは 2 形式に対応: 新形式 `<session>/subagents/agent-*.jsonl` + `.meta.json`（`toolUseId` を `Lane::spawned_by` に入れる）と、旧形式のプロジェクト直下 `agent-*.jsonl`（`Loaded::Child` として親セッション ID 付きで返す）。
  - ツール呼び出しと結果は `Pending`（call_id → イベント index）で突き合わせ、結果のタイムスタンプで `end` を確定する。テキストは `MAX_TEXT` で切り詰める。
  - 結果から呼び出しの成否（`Status`: 失敗・拒否・中断・結果なし）を判定する。Claude は `is_error` と `toolUseResult`、Codex は終了コード・定型文（`Script failed` など）と、exec の中の各呼び出しの結果を見る。
  - 入力は詳細表示で読みやすい形にして保存する: シェルは `$ コマンド`、Claude の Edit / Write は `*** Update File: パス` 形式の差分（結果の `structuredPatch` があれば行番号付きの差分に置き換える）、TodoWrite は計画の一覧、Agent は指示文。
  - Git のブランチと CLI のバージョン、Codex のゴール（`/goal`。ユーザーの発話として扱い、タイトルがなければタイトルにする）も拾う。
- **jsmock.rs** — Codex のコードモード (`exec`) の JS を QuickJS (rquickjs) で実行し、`tools` を Proxy にしたモックで呼び出しを記録する。読み込み時は静的解析 (`js_tool_calls`) で名前と要約だけを決め、input には JS を `JS_MARK` 付きで保存する。詳細表示時に `loader::exec_detail` で整形し、結果を `jscache.db`（キーは JS 本文のハッシュ）にキャッシュする。モックの挙動を変えたら `ENGINE_VERSION` を上げる。
- **model.rs** — `Session` は複数の `Lane`（main + サブエージェント）を持ち、`Lane` は時刻順の `Event` 列。`Event` は結果の状態 (`status`) と API 呼び出しのトークン使用量 (`usage` / `model`) を持ち、`Stats`（ターン・ツール・失敗の数）と `UsageSum`（トークン・料金）で集計する。`Store` はレーンを読み込み元ファイル (`Lane::origin`) 単位で差し替え、親より先に届いた子レーンを `orphans` に保持して親セッション追加時に合流させる。描画用の派生データ（`cum_end`, `parents`, `spawn_t`）は `Session::finalize` で事前計算し、描画中に全イベントを走査しない。時刻はすべて Unix 秒の `f64`。
- **pricing.rs** — モデルごとの料金表（100 万トークンあたりの USD）とトークン使用量の料金換算。`~/Library/Application Support/rireki/prices.json` で追加・上書きできる。
- **layout.rs** — 表示スケール（`spp` = 1px あたり秒数）と `Granularity`（自動／同一ツール／ターン／コール）に応じて、レーンのイベントを `Block`（events の index 範囲）に集約する。表示範囲外のブロックは捨てる。
- **app.rs** — GPUI の `Rireki` ビュー。状態、データの反映、選択と履歴（戻る / 進む）、表示位置の移動（`reveal`）、ホイール・ドラッグ・ピンチ・境界ドラッグの入力処理、画面全体の組み立てを担う。画面は Slack 風に、上端の帯・左端のアイコンレール・サイドバー・メイン列・詳細パネルを並べる。描画は `app/` 以下に分ける:
  - `rail.rs` — 上端の帯と、読み込み元（すべて / Claude / Codex）・表示（タイムライン / カレンダー）を切り替えるレール
  - `sidebar.rs` — 検索欄と、進行中・プロジェクト・セッションの一覧、または検索結果。`SidebarView` として別ビューにし、一覧が変わったときだけ描き直す（`sidebar_changed`）
  - `header.rs` — メイン列の見出し（期間の移動・表示範囲・拡大縮小・まとめ方）と下端の凡例。幅が狭いと項目を減らす
  - `timeline.rs` / `calendar.rs` — タイムラインとカレンダー（日 / 週）。ブロックは 1 つの canvas に描き、当たり判定用に位置を覚える
  - `range.rs` — 期間を選ぶカレンダー（ポップオーバー、日ごとのイベント量を棒で表示）と、タイムライン上端のミニマップ（選んだ期間、未選択なら全期間のイベント分布と現在の表示範囲。クリック・ドラッグで移動）
  - `detail.rs` — 詳細パネル（`DetailView`）。選択している間だけ開く。見出しに結果の状態と前後のイベントへの移動（J / K）、セッションは 1 行の要約（開くと期間・ブランチ・トークンと料金・集計）。1 イベントなら時刻・トークン・きっかけの指示と入力・結果、複数なら集計と、名前・失敗・編集したファイルで絞り込めるイベント一覧（ユーザーの指示ごとに区切る）。本文は選択時に `load_detail` で DB から引いて `DetailCache` に持つ
  - `rich.rs` — 詳細パネルの本文の表示: Markdown（メッセージ・計画・サブエージェントの報告）、行番号付きの差分、ANSI の色、`cat -n` 形式の行番号、YAML 風の引数のキーの色分け。解析（テストあり）と描画を分けてある
  - `overlay.rs` — ホバーカード・まとめ方メニュー・ショートカット一覧
  - `keymap.rs` — アクション・キー割り当て・メニューバー。メニューには最初に登録したキーが出るので修飾キー付きを先に登録し、1 文字のキーは `Rireki && !TextInput` で検索欄の入力中は効かせない
  - `widgets.rs` — ボタンやツールチップなどの共通部品
- GPUI の注意点:
  - タイトルバーを透明にしている。上端の約 28px（macOS のタイトルバー領域）はドラッグでウィンドウ移動・ダブルクリックでズームになるので、上端の帯 (`STRIP_H`) には操作する部品を置かない。
  - gap を付けた行の子に `ml_auto()` を使うと gap が効かなくなる。右寄せは `div().flex_1()` の詰め物で行う。
  - 内容パネルは角丸なので、角に接する子要素は背景を塗らないか `INNER_RADIUS` で角を丸める（塗ると枠線を覆う）。
- **search.rs** — サイドバーの検索。セッションのタイトル・ディレクトリ・ID とイベントの名前・要約を部分一致で探す。ASCII だけ小文字にした本文をセッションごとに作り（更新されたセッションだけ作り直す）、memmem で走査する。
- **input.rs** — 検索欄の 1 行テキスト入力（IME の変換中文字列・横スクロール対応）。
- **prefs.rs** — 画面の状態（表示モード・パネルの開閉と幅・まとめ方・絞り込み・折りたたみ・詳細パネルのセッション情報の開閉）を `~/Library/Application Support/rireki/prefs.json` に保存し、起動時に復元する。
- **theme.rs** — 配色と、ツール名 → 色のマッピング（Claude と Codex のツール名を同じカテゴリにまとめる）。新しいツール名を色分けする場合は `tool_color` と `LEGEND` を合わせて更新する。

コメントや UI 文言は日本語で書かれている。
