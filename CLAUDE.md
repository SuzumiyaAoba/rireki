# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 概要

rireki は Claude Code / Codex のセッションログ (JSONL) を読み込み、ツール呼び出しやメッセージをタイムライン／カレンダー表示する macOS 向けデスクトップアプリ。Rust (edition 2024) + GPUI 製。

## コマンド

```bash
cargo build                 # ビルド
cargo run                   # GUI 起動
cargo run -- --stats        # GUI を起動せず、索引 DB の読み込みと差分取り込みを行い、件数と所要時間を表示
cargo run --release -- --bench  # 大量イベント・検索・長文整形を JSON Lines で計測
cargo run --release -- --bench path/to/index.db  # 読み取り専用の索引と大きいログの計測も加える
cargo run --release -- --bench-discover claude/projects codex/sessions  # 固定したルートの探索と指紋を比較
cargo run --release -- --bench-view path/to/index.db  # 並べ替えと非対話セッションの起動元探索を計測
cargo run --release -- --verify-logs  # 大きい実ログの解析結果を本文なしのハッシュで確認
cargo run --release -- --bench-ingest manifest.json  # 空の RIREKI_CACHE_DIR へ全件取り込み・再読込の検証
cargo clippy                # lint
cargo test                  # テスト
```

`.cargo/config.toml` で linker / CC を `/usr/bin/cc` に固定している（Nix の cc が libiconv を見つけられないため）。変更しないこと。

`RIREKI_CACHE_DIR` と `RIREKI_CONFIG_DIR` でキャッシュと設定の保存先を分けられる。`--profile-ui` は両方を隔離したディレクトリに指定して使うネイティブ画面計測で、コピーした索引だけを読み、ログの差分取り込みを止める。`--profile-detail` を併用すると詳細のスクロール位置・絞り込み・幅変更・タブ・メモリ内の模擬更新を検証する。OS 入力の注入は行わず、公開 API を使って位置と選択を変える。`ZED_MEASUREMENTS=1` で GPUI のフレーム処理の経過時間も記録する。`--bench-ingest` は空のキャッシュを要求し、manifest に固定した安定ログをパース・保存し、本文を含めて再読み込みと比較する。manifest はログのパスを含むため公開しない。

## アーキテクチャ

起動計測は `--profile-ui --profile-startup`。`main_to_ready_ms` は main の開始から、データを含むフレームの描画・提示後、次のフレーム要求までの時間。OS のローダーと GPU 完了時間は含まない。`--profile-ingest manifest.json` を加えると空の隔離キャッシュから通常の取り込み経路を動かす。`--profile-idle claude/projects codex/sessions 15` は指定したルートを 3 秒ごとに探索し、指定秒数待つ。全ログが索引済みでなければパース前に停止する。`--profile-rich` は差分展開・単一長文・24 時間の日週表示を検証する。いずれも隔離したキャッシュと設定を指定する。

`--profile-ui --profile-files` はファイル履歴の展開・幅変更・並びの更新で同じ編集行を保ち、その編集を開くことを検証する。画面ロックで実画面のフレーム要求が止まる場合、`--profile-render` を併用して公開 API の `Window::draw` と要素の解放を測れる。この値は描画命令の構築・レイアウト・paint を含み、GPU への提示は含まない。`--profile-eager-files` は計測専用で、同じ行構造をすべて組み立てる比較用の方式に切り替える。通常のファイル一覧は可視行だけを作る。

データの流れ: 背景の `index::ViewLoader` が索引 DB の読み込み、`Indexer::plan` / `process`、`model::Store::replace` と描画用の索引化を行う。画面と検索は `model::Snapshot`（`Arc<Session>` の集合）を受け取り、更新前の版は読み終わるまで保つ。3 秒ごとにリアルタイムファイルを再読込する。

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
- **layout.rs** — 表示スケール（`spp` = 1px あたり秒数）と `Granularity`（自動／同一ツール／ターン／コール）に応じて、レーンのイベントを `Block`（events の index 範囲）に集約する。表示範囲外のブロックは捨てる。`Cache` は自動／同一ツールの集約をスケールごとに再利用し、名前ごとのイベント位置でブロックの色とラベルを決める。セッション更新時にキャッシュを破棄する。
- **app.rs** — GPUI の `Rireki` ビュー。状態、データの反映、選択と履歴（戻る / 進む）、表示位置の移動（`reveal`）、ホイール・ドラッグ・ピンチ・境界ドラッグの入力処理、画面全体の組み立てを担う。画面は Slack 風に、上端の帯・左端のアイコンレール・サイドバー・メイン列・詳細パネルを並べる。描画は `app/` 以下に分ける:
  - `rail.rs` — 上端の帯と、読み込み元（すべて / Claude / Codex）・表示（タイムライン / カレンダー）を切り替えるレール
  - `sidebar.rs` — 検索欄と、進行中・プロジェクト・セッションの一覧、または検索結果。`SidebarView` として別ビューにし、一覧が変わったときだけ描き直す（`sidebar_changed`）。検索結果は高さの異なる行に対応した GPUI の `ListState` で、見えている項目だけ作る
  - `header.rs` — メイン列の見出し（期間の移動・表示範囲・拡大縮小・まとめ方）と下端の凡例。幅が狭いと項目を減らす
  - `timeline.rs` / `calendar.rs` — タイムラインとカレンダー（日 / 週）。ブロックは 1 つの canvas に描き、当たり判定用に位置を覚える。装飾のない連続した矩形は順序を保って `paint_layer` にまとめ、GPUI の境界木への登録を減らす
  - `view_index.rs` — 非対話セッションの起動元を作業ディレクトリとツールの時刻から求める。派生索引 `Lane::cum_tool_end` で候補を絞り、長時間の Agent が探索範囲を広げないようにする。DB には保存しない
  - `range.rs` — 期間を選ぶカレンダー（ポップオーバー、日ごとのイベント量を棒で表示）と、タイムライン上端のミニマップ（選んだ期間、未選択なら全期間のイベント分布と現在の表示範囲。クリック・ドラッグで移動）
  - `detail.rs` — 詳細パネル（`DetailView`）。選択している間だけ開く。見出しに結果の状態・レーン内の位置と前後のイベントへの移動（J / K）。その下の文脈の帯に「読み込み元 › プロジェクト › セッション › サブエージェント」のパンくず（セッション名でセッションのタブへ、矢印で起動元へ）。その下のタブ（`prefs::DetailTab`、1 / 2 / 3）で「イベント」「ファイル」（編集したファイルと追加・削除の割合。押すとその場で開き、編集の時系列の帯と編集ごとの差分を出す）「セッション」（集計のタイル・操作・サブエージェントの一覧・トークンと料金・属性）を切り替える。イベントのタブは 1 イベントなら時刻・トークン・きっかけの指示と入力・結果、複数なら集計のタイル（失敗は押すと絞り込み）と、名前・失敗で絞り込めるイベント一覧（ユーザーの指示ごとに区切る）。本文は選択時に `request_detail` で背景処理へ渡し、`DetailLoader` が DB 読み込み・JS の整形を行って `DetailCache` に持つ。古い選択の結果は世代で破棄する。複数イベントの一覧は `DetailList` の可変高リストで可視行だけ作り、幅変更や更新では上端のイベントと行内位置を保つ。先頭 300 件に絞り込みを掛ける順序は変えない。末尾にいた場合と追従アニメーション中は連続更新を追い、ユーザーのスクロールや絞り込み・タブ変更で追従を中断する
  - `rich.rs` — 詳細パネルの本文の表示: Markdown（メッセージ・計画・サブエージェントの報告）、行番号付きの差分、ANSI の色、`cat -n` 形式の行番号、YAML 風の引数のキーの色分け。解析（テストあり）と描画を分けてある
  - `detail/files.rs` — ファイル一覧・編集履歴の可変高リスト。ファイルの見出し、編集の帯、各編集と展開ボタンを別の項目にし、表示範囲だけ組み立てる。選択全体の編集を保持し、先頭 100 ファイルと編集ごとの 120 行の折りたたみは従来どおり。幅・読み込み世代・ファイル順序が変わっても、パスとイベント位置から上端の編集と行内位置を引き継ぐ
  - `overlay.rs` — ホバーカード・まとめ方メニュー・ショートカット一覧
  - `keymap.rs` — アクション・キー割り当て・メニューバー。メニューには最初に登録したキーが出るので修飾キー付きを先に登録し、1 文字のキーは `Rireki && !TextInput` で検索欄の入力中は効かせない
  - `widgets.rs` — ボタンやツールチップなどの共通部品
- GPUI の注意点:
  - タイトルバーを透明にしている。上端の約 28px（macOS のタイトルバー領域）はドラッグでウィンドウ移動・ダブルクリックでズームになるので、上端の帯 (`STRIP_H`) には操作する部品を置かない。
  - gap を付けた行の子に `ml_auto()` を使うと gap が効かなくなる。右寄せは `div().flex_1()` の詰め物で行う。
  - 内容パネルは角丸なので、角に接する子要素は背景を塗らないか `INNER_RADIUS` で角を丸める（塗ると枠線を覆う）。
- **search.rs** — サイドバーの検索。セッションのタイトル・ディレクトリ・ID とイベントの名前・要約を部分一致で探す。ASCII だけ小文字にした本文をセッションごとに作り、memmem で背景から走査する。キャッシュは弱参照でセッションの版を識別し、古い本文を保持しない。世代が変われば走査を中断し、最新の結果だけ反映する。
- **input.rs** — 検索欄の 1 行テキスト入力（IME の変換中文字列・横スクロール対応）。
- **prefs.rs** — 画面の状態（表示モード・パネルの開閉と幅・まとめ方・絞り込み・折りたたみ・詳細パネルのタブ）を `~/Library/Application Support/rireki/prefs.json` に保存し、起動時に復元する。
- **theme.rs** — 配色と、ツール名 → 色・アイコンのマッピング（Claude と Codex のツール名を同じカテゴリにまとめる）。色はカテゴリ単位、アイコンはツール単位。新しいツール名を扱う場合は `tool_color`・`tool_icon`・`LEGEND` を合わせて更新する。
- **icons.rs** — UI アイコン（Lucide）とイベントの種類のアイコン（Iconify の lucide セットから取得した SVG 本体）を埋め込み、`AssetSource` として提供する。

コメントや UI 文言は日本語で書かれている。
