# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 概要

rireki は Claude Code / Codex のセッションログ (JSONL) を読み込み、ツール呼び出しやメッセージをタイムライン／カレンダー表示する macOS / Linux / Windows 向けデスクトップアプリ。Rust (edition 2024) + GPUI 製。

## コマンド

```bash
cargo build                 # ビルド
cargo run                   # GUI 起動
cargo run -- --stats        # GUI を起動せず、索引 DB の読み込みと差分取り込みを行い、件数と所要時間を表示
cargo run --release -- --bench  # 大量イベント・検索・長文整形を JSON Lines で計測
cargo run --release -- --bench path/to/index.db  # 読み取り専用の索引と大きいログの計測も加える
cargo run --release -- --bench-search path/to/index.db  # 初回・再利用・中断の検索を測定し、全件走査と照合
cargo run --release -- --bench-db path/to/index.db  # 固定した索引DBの読み込みと内容ハッシュだけを計測
cargo run --release -- --bench-db-memory path/to/index.db  # macOS: 15回の読み込み・破棄前後の使用ヒープと確保領域を記録
cargo run --release -- --bench-discover claude/projects codex/sessions  # 固定したルートの探索と指紋を比較
cargo run --release -- --bench-statistics path/to/index.db  # 統計の新規集計・再利用・単一セッション更新を計測
cargo run --release -- --bench-range path/to/index.db  # ミニマップと期間カレンダーの分布集計を計測
cargo run --release -- --bench-summary path/to/index.db  # 詳細の選択・セッション集計と再利用を計測
cargo run --release -- --bench-details path/to/index.db path/to/texts.db  # 詳細本文の読み込みを内容ハッシュと所要時間で比較
cargo run --release -- --bench-view path/to/index.db  # 並べ替えと非対話セッションの起動元探索を計測
cargo run --release -- --verify-logs  # 大きい実ログの解析結果を本文なしのハッシュで確認
cargo run --release -- --bench-ingest manifest.json  # 空の RIREKI_CACHE_DIR へ全件取り込み・再読込の検証
cargo run --release -- --bench-ingest manifest.json --measure-memory  # macOS: 取り込み中の使用ヒープを2ms間隔で記録
cargo run --release -- --bench-parse manifest.json  # 各形式の大きい固定ログを、本文の破棄まで7回ずつ計測
cargo run --release -- --bench-parse manifest.json --measure-memory  # macOS: 初回と反復解析のピークヒープ・破棄後の確保領域も記録
scripts/bundle.sh           # リリースビルドから target/release/rireki.app を作る（assets/app-icon のアイコンと Info.plist）
cargo clippy                # lint
cargo test                  # GUI を含むテスト
cargo test --no-default-features  # GUI の依存を外してコア・CLI・統合テストを実行
cargo run --no-default-features -- --stats  # GUI の依存なしで統計コマンドを実行
```

`.cargo/config.toml` で linker / CC を `/usr/bin/cc` に固定している（Nix の cc が libiconv を見つけられないため）。変更しないこと。

`RIREKI_CACHE_DIR` と `RIREKI_CONFIG_DIR` でキャッシュと設定の保存先を分けられる。`--profile-ui` は両方を隔離したディレクトリに指定して使うネイティブ画面計測で、コピーした索引だけを読み、ログの差分取り込みを止める。`--profile-detail` を併用すると詳細のスクロール位置・絞り込み・幅変更・タブ・メモリ内の模擬更新を検証する。OS 入力の注入は行わず、公開 API を使って位置と選択を変える。`ZED_MEASUREMENTS=1` で GPUI のフレーム処理の経過時間も記録する。`--bench-ingest` は空のキャッシュを要求し、manifest に固定した安定ログをパース・保存し、本文を含めて再読み込みと比較する。manifest はログのパスを含むため公開しない。`RIREKI_BENCH_LOG_ROOT` を指定すると、その中の `claude` / `codex` ディレクトリを同じ探索処理で読み、隔離したログのコピーで再現できる。

## アーキテクチャ

`src/lib.rs` が GUI に依存しないコアの入口。`history` / `catalog` / `model` / `loader` / `db` / `index` / `search` / `pricing` / `jsmock` / `layout` に加え、`statistics` / `detail` / `patch` / `histogram` / `view_index` / `time` / `mcp` を公開する。`src/cli.rs` が画面を開かない計測・統計コマンドを振り分け、`src/main.rs` は feature に応じて GUI を起動する。`main.rs` と `app/` はこのライブラリを参照し、GPUI の状態・入力・描画・背景タスクの起動と結果反映を管理する。コアから `app` / `theme` / `icons` / GPUI を参照しない。通常起動は既定の `gui` feature、コア・CLI の利用と検証は `--no-default-features`。TUI の画面と入力は未実装。

- **history.rs / catalog.rs** — `History` が索引・Store を所有し、キャッシュ読み込みと差分取り込みを行う。`ingest` は64ファイルごとに進捗と Snapshot を返し、破棄すると未処理分を次回の探索へ残す。GUI は一つの `process` を背景で実行して反映を待つ。実行器・タイマー・選択・描画は各フロントエンドの責務。`Catalog` は source で絞る scope、cwd も絞る order、scope ごとのプロジェクトを返す。`SourceFilter` の保存形式は従来どおり。`index::ViewLoader` は `History` の互換名。

- **statistics.rs / statistics/cache.rs** — 統計の期間（7 / 30 / 90 日・全期間）・結果型と、セッションの版を弱参照で識別する集計キャッシュ。直前の同じ長さの期間の合計、読み込み元ごとの列（全期間は 92 日を超えると週、371 日を超えると月ごと）、曜日 × 時刻のマスと曜日・時ごとの合計（セッションは重ねずに数える）、ツール名ごとの呼び出しと失敗も集める。日付と時は各日の午前 0 時の表とレーン内のカーソルで求め、境界の直前が前日・境界が当日にならない日（曖昧な午前 0 時など）があればイベントごとに変換する。テストは全イベントを日時変換する参照実装と比べる。GUI は同じキャッシュを保持し、背景で実行する。集計キャッシュの詳細は `docs/perf.md`。
- **detail.rs / detail/** — `detail.rs` は選択範囲と結果型の入口。`loading.rs` は本文の遅延読み込み・集計キャッシュ・キャンセル、`content.rs` は保存状態をまたぐ本文復元と JS・コマンド整形、`files.rs` は差分とファイル履歴、`summary.rs` は選択とセッションの集計を担当する。色やアイコンを含めず、名前ごとの先頭イベント位置を返す。GUI はそのイベントの表示を決める。キャンセル確認と本文の表示上限を維持する。
- **detail/analysis.rs** — 選択範囲全体の分析とメタデータのJSON出力。検索語は名前・要約・モデルをまたいで照合する。小文字化は名前から順に行い、前のフィールドで一致すれば後の処理を省く。各イベント内で必要になった要約・モデルだけを一度作る。料金は同じモデルが続く間だけ共有し、P95は全件のソートをせず必要な順位の値を選ぶ。使用量・料金の加算順と同順位の並びを維持する。
- **model.rs / loader.rs / db.rs** — 公開 API は各ルートで再公開し、型と処理の実体は責務別の子モジュールへ置く。 イベント名は `Arc<str>` で共有する。ログのパースはファイルごと、索引の並列読み込みはID範囲ごとに同じ名前を再利用し、メタデータの複製とGPUIの表示でも共有する。DBの値と保存形式は変更しない。要約と呼び出しIDも同様に共有する。読み取りの詳細は `docs/perf.md`。
- **text.rs / model::Event::text** — 安定ログの保存待ちの本文はファイルごとの圧縮データを共有し、成功したコミット後にDBの行IDへ置き換える。イベント本文を取得するときは `Event::text()` を使用する。`Event::text_ref` は行IDと保存状態を一つの領域に収め、通常のDB読み込みでもイベントのサイズを増やさない。
- **index.rs / index/** — `index.rs` は探索・取り込み・保存指紋の調整、`writer.rs` は非同期保存と完了通知・終了処理、`live_cache.rs` はライブ解析状態の容量と保持順・並列回収上限、`history.rs` は Store への反映と Snapshot の公開を担当する。低水準の探索・解析 API は `index` から公開する。保存待ちの指紋は保存済みと区別し、成功した書き込みだけを保存済みにする。失敗時は次の探索で再試行し、古い保存完了が新しい版やリアルタイムの指紋を書き換えない。安定ログは並列ワーカー内でファイルごとにパース・圧縮・本文解放まで連続して行い、未圧縮本文の一時保持を作業中のファイルに限定する。画面反映とDB書き込みは64ファイル単位を維持する。
- **search.rs** — 初回の検索索引の長さ計算・連結・小文字化は256イベントごとに中断を確認する。中断した途中の索引は公開せず、完成したセッションの版だけを再利用する。候補の保持と名前の共有の詳細は `docs/perf.md`。
- **perf.rs / perf/** — コアの公開 API を直接使う CLI の計測。画面用のモジュールを経由しない。`perf/ingest.rs` に固定ログの取り込みと往復検証、`fingerprint.rs` に内容ハッシュ、`verification.rs` にログの照合、`db.rs` に読み取り計測、`view.rs` に表示順と起動元探索の計測を置く。取り込みの比較用の全件パースと内容照合は取り込み時間の外で行い、Storeの共有された版からも保存後の本文とイベントの対応を検証する。

各モジュールの性能上の不変条件と `--profile-*` の計測内容は `docs/perf.md` にまとめている。該当モジュールを変更するときは先に読み、記載の不変条件を維持して計測で比較すること。

データの流れ: 背景の `history::History` が索引 DB の読み込み、`Indexer::plan` / `process`、`model::Store::replace` と描画用の索引化を行う。画面と検索は `model::Snapshot`（`Arc<Session>` の集合）を受け取り、更新前の版は読み終わるまで保つ。3 秒ごとにリアルタイムファイルの変更を探索し、変更されたログを照合・差分解析する。

- **index.rs / loader/live.rs** — ファイルを「安定」（最終更新から `LIVE_SECS` 以上経過）と「リアルタイム」に分ける。安定ファイルはパース結果を DB に保存し、指紋（mtime + サイズ）が変わるまで読み直さない。リアルタイムでは改行まで確定した行の解析状態を保持する。既読部分の64KiBブロックのXXH3の128bitチェックサムと残りのバイト列を、256KiBの読み出しバッファで照合する。読み出しバッファ内の64KiB区間は直接検証し、短い読み出しだけ作業領域へ連結する。チェックサムの作成も、連続した64KiB区間は直接行う。その後に追記分だけを解析し、書き換え・切り詰めを検出したら全体を読み直す。改行前の最終行は公開版だけに反映し、次回はその行から読み直す。新規の本文と、結果が届いた古い本文を追跡し、256件たまるか更新を読み終えた時点で圧縮して共有する。本文を圧縮するために全イベントを毎回走査しない。公開版は表示に必要なレーン・メタデータ・使用量だけを複製し、解析用マップは保持元に残す。改行前の行を読む分岐には解析状態全体を複製する。生のイベント順と保留中の呼び出し・累計使用量を保持し、ソートとClaudeのmessage.id単位の使用量反映は公開版で行う。Claudeの各サブエージェントも個別に保持し、メタデータは毎回読み直す。解析状態は保守的な容量見積もりで合計64MiBを上限とし、古いものから破棄する。並列解析で回収待ちの状態も1バッチ64MiBまでに制限する（処理中の一時領域や画面が保持する版は別）。削除されたファイルは次の探索で保持対象から外す。上限を超える単一ログは更新ごとに全体解析へ戻る。更新が止まると保持した状態で最終版を作って安定側へ移り、共有済みの圧縮本文は再圧縮せずDBへ渡す。`Indexer::open_with_live_cache(false)` / `ViewLoader::open_with_live_cache(false)` で同じ更新・保存経路を全体解析と比較できる。
- **db.rs** — `db.rs` は公開型と保存先の入口とし、`db/connection.rs` に接続とスキーマ、`codec.rs` に保存値の変換、`write.rs` に書き込みとコミット後の本文更新、`read.rs` に索引の復元と本文取得を置く。`packing.rs` は本文の圧縮と保存状態、`events.rs` はイベントの並列読み取りと要約共有、`js_cache.rs` は JS 整形キャッシュを担当する。索引 DB (`~/Library/Caches/rireki/index.db`, SQLite/rusqlite)。`files` / `items` / `lanes` / `events` / `texts`。起動時は `events` だけを ID 範囲で分割して並列に読み、大きな input / output (`texts`) は詳細表示のときに `Event::row()` で引く。`texts` は lz4 圧縮して別ファイル `texts.db` に置く（同じファイルだと `events` のページが散らばり起動が遅くなる）。書き込みは `Indexer` の専用スレッドで行い、次のチャンクのパースと重ねる。パース結果やスキーマを変えたら `VERSION` を上げる（DB が作り直される）。
- **loader.rs** — 読み込み元は `~/.claude/projects/*/*.jsonl` と `$CODEX_HOME/sessions`（既定 `~/.codex/sessions`、再帰探索）。ファイルは mtime の新しい順に `Job` 化される。
  - `loader/discovery.rs` は探索と指紋、`loader/log_lines.rs` は行読み出しとJSON解析、`loader/format.rs` は共通整形とClaudeの編集差分・計画、`loader/codex_tools.rs` はCodexのツール入出力とJSの整形を担当する。形式ごとの解析状態とイベントの対応付けは `loader/claude.rs` と `loader/codex.rs` に置き、共通の保留呼び出し・共有文字列は `loader.rs`、差分解析は `loader/live.rs` が管理する。公開APIは `loader` 経由で提供する。
  - JSONのYAML風表示は行を順に生成し、6,000文字の一文字先で止めて従来と同じ位置に省略記号を付ける。長い文字列・キーも必要な先頭だけを複製する。配列の1行表示は80文字を超えると判定を止め、全文を連結しない。日本語・字下げ・空の値・改行の扱いを維持する。
  - Claude のサブエージェントは 2 形式に対応: 新形式 `<session>/subagents/agent-*.jsonl` + `.meta.json`（`toolUseId` を `Lane::spawned_by` に入れる）と、旧形式のプロジェクト直下 `agent-*.jsonl`（`Loaded::Child` として親セッション ID 付きで返す）。
  - ツール呼び出しと結果は `Pending`（call_id → イベント index）で突き合わせ、結果のタイムスタンプで `end` を確定する。テキストは `MAX_TEXT` で切り詰める。
  - 結果から呼び出しの成否（`Status`: 失敗・拒否・中断・結果なし）を判定する。Claude は `is_error` と `toolUseResult`、Codex は終了コード・定型文（`Script failed` など）と、exec の中の各呼び出しの結果を見る。
  - 入力は詳細表示で読みやすい形にして保存する: シェルは `$ コマンド`、Claude の Edit / Write は `*** Update File: パス` 形式の差分（結果の `structuredPatch` があれば行番号付きの差分に置き換える）、TodoWrite は計画の一覧、Agent は指示文。
  - Git のブランチと CLI のバージョン、Codex のゴール（`/goal`。ユーザーの発話として扱い、タイトルがなければタイトルにする）も拾う。
- **jsmock.rs** — Codex のコードモード (`exec`) の JS を QuickJS (rquickjs) で実行し、`tools` を Proxy にしたモックで呼び出しを記録する。読み込み時は静的解析 (`js_tool_calls`) で名前と要約だけを決め、input には JS を `JS_MARK` 付きで保存する。詳細表示時に `loader::exec_detail` で整形し、結果を `jscache.db`（キーは JS 本文のハッシュ）にキャッシュする。モックの挙動を変えたら `ENGINE_VERSION` を上げる。
- **model.rs** — `model/event.rs` はイベントと結果、`usage.rs` は使用量と飽和演算、`session.rs` はレーンと親子関係、`store.rs` は差し替えとスナップショット、`hashing.rs` は内部セッション番号専用ハッシュを管理する。`Session` は複数の `Lane`（main + サブエージェント）を持ち、`Lane` は時刻順の `Event` 列。`Event` は結果の状態 (`status`) と API 呼び出しのトークン使用量 (`usage` / `model`) を持ち、`Stats`（ターン・ツール・失敗の数）と `UsageSum`（トークン・料金）で集計する。`Store` はレーンを読み込み元ファイル (`Lane::origin`) 単位で差し替え、親より先に届いた子レーンを `orphans` に保持して親セッション追加時に合流させる。描画用の派生データ（`cum_end`, `parents`, `spawn_t`）は `Session::finalize` で事前計算し、描画中に全イベントを走査しない。時刻はすべて Unix 秒の `f64`。
- **cost_timeline.rs** — 表示期間内のAPI換算料金を、セッション・ディレクトリ・時間区間で集計する。時刻順のイベントは、同じ区間の間だけ集計先を再利用して木構造の検索を省く。各呼び出しの加算順と最大料金の代表イベントを保ち、256イベントごとのキャンセル確認を続ける。 ディレクトリの合計は512区間まで固定サイズの位置索引で併合し、処理した位置だけリセットする。1セッションなら区間一覧を複製し、出力の余剰容量を抑える。 セッション番号の重複除去と保持判定は、有効な番号が広く含まれるときにビット配列を使い、少数の対象や不正な番号を含む場合はHashSetを使う。
- **pricing.rs** — モデルごとの料金表（100 万トークンあたりの USD）とトークン使用量の料金換算。`~/Library/Application Support/rireki/prices.json` で追加・上書きできる。 最長のキーから照合する。集計中の料金検索は先頭候補以外をモデル名の先頭バイトで絞り、料金表への参照を共有する。モデル名の表は256件まで保持し、直前の1モデルは別に持つ。モデルなしは照会しない。
- **layout.rs** — 表示スケール（`spp` = 1px あたり秒数）と `Granularity`（自動／同一ツール／ターン／コール）に応じて、レーンのイベントを `Block`（events の index 範囲）に集約する。表示範囲外のブロックは捨てる。`Cache` は自動／同一ツールの集約を表示範囲の近傍に保持し、同一ツールでは結合条件が変わらない縮尺でも再利用する。名前ごとのイベント位置でブロックの色とラベルを決める。セッション更新時にキャッシュを破棄する。
- **app.rs** — GPUI の `Rireki` ビューの状態と画面全体の組み立てを担う。`app/init.rs` は初期化と背景読み込み、`data.rs` はデータ反映と表示対象、`selection.rs` は詳細の選択・閲覧履歴・`reveal`、`navigation.rs` は表示と期間の移動、`search_controller.rs` は検索の入力購読・遅延実行・世代キャンセルと、入力・索引・タスク・結果・一覧キャッシュをまとめた `State`、`interaction.rs` は入力、`panes.rs` は寸法とリサイズ、`format.rs` は表示用の整形を管理する。画面は Slack 風に、上端の帯・左端のアイコンレール・サイドバー・メイン列・詳細パネルを並べる。描画は `app/` 以下に分ける:
  - `rail.rs` — 上端の帯と、読み込み元（すべて / Claude / Codex）・表示（タイムライン / カレンダー）を切り替えるレール
  - `sidebar.rs` — 検索欄と、進行中・プロジェクト・セッションの一覧、または検索結果。`SidebarView` として別ビューにし、一覧が変わったときだけ描き直す（`sidebar_changed`）。検索結果は高さの異なる行に対応した GPUI の `ListState` で、見えている項目だけ作る
  - `header.rs` — メイン列のツールバーと下端の凡例。左に移動（前 / 今日 / 次）と表示中の期間（押すと期間・日付を選ぶポップオーバー、G）、右に絞り込みのチップ・拡大縮小・表示の切り替え（イベント / API コスト、カレンダーでは日 / 週 / 月）・表示オプション。部品の高さは `CONTROL_H` にそろえる。幅は推定幅（`Fit`）で決め、期間の表記を短くし、表示の切り替えを表示オプションのメニューへ移す順に縮める。部品を黙って消さず、期間の表記は最後まで残す
  - `calendar.rs` — カレンダー（日 / 週 / 月）。日・週はセッション（サブエージェントと起動した子を含む）が動いていた区間を 1 ブロックにし、上端を開始時刻にそろえ、プロジェクトの色で名前を枠内に書く。幅があれば指示を時刻の位置に並べ、押すとそのターンを開く。重なるブロックは横に並べ、右の列が空いていれば広げる。日の見出しに稼働時間（重ねずに数える）と件数、下端にプロジェクト別の稼働時間を出す。期間を移って作業が見えなければ最初の作業までスクロールする。ダブルクリックでその範囲をタイムラインで開く。月はその日のセッションを開始順に並べ、稼働時間を濃淡で示す。集計は表示中の期間に動いていたレーンだけで行う（「範囲内のみ」に依存しない）
  - `timeline.rs` — タイムライン。ブロックは 1 つの canvas に描き、当たり判定用に位置を覚える。装飾のない連続した矩形は順序を保って `paint_layer` にまとめ、GPUI の境界木への登録を減らす
  - `range.rs` — 期間の表記から開くポップオーバー（タイムラインでは直近の期間の一覧と、日ごとのイベント量を棒で示すカレンダーで日付の範囲を選ぶ。カレンダーの表示では押した日へ移動）と、タイムライン上端のミニマップ（選んだ期間、未選択なら全期間のイベント分布と現在の表示範囲。クリック・ドラッグで移動）
  - `detail.rs` — 詳細パネル（`DetailView`）。選択している間だけ開く。見出しに結果の状態・レーン内の位置と前後のイベントへの移動（J / K）。その下の文脈の帯に「読み込み元 › プロジェクト › セッション › サブエージェント」のパンくず（セッション名でセッションのタブへ、矢印で起動元へ）。その下のタブ（`prefs::DetailTab`、1 / 2 / 3）で「イベント」「ファイル」（編集したファイルと追加・削除の割合。押すとその場で開き、編集の時系列の帯と編集ごとの差分を出す）「セッション」（集計のタイル・操作・サブエージェントの一覧・トークンと料金・属性）を切り替える。イベントのタブは 1 イベントなら時刻・トークン・きっかけの指示と入力・結果、複数なら集計のタイル（失敗は押すと絞り込み）と、名前・失敗で絞り込めるイベント一覧（ユーザーの指示ごとに区切る）。本文は選択時に `request_detail` で背景処理へ渡し、コアの `detail::DetailLoader` が DB 読み込み・JS の整形を行って `DetailCache` に持つ。古い選択の結果は世代で破棄する。複数イベントの一覧は `DetailList` の可変高リストで可視行だけ作り、幅変更や更新では上端のイベントと行内位置を保つ。先頭 300 件に絞り込みを掛ける順序は変えない。末尾にいた場合と追従アニメーション中は連続更新を追い、ユーザーのスクロールや絞り込み・タブ変更で追従を中断する
  - `rich.rs` — 詳細パネルの本文の表示: Markdown（メッセージ・計画・サブエージェントの報告）、行番号付きの差分、ANSI の色、`cat -n` 形式の行番号、YAML 風の引数のキーの色分け。Markdownの通常文は装飾記号までまとめてコピーし、区切り線は文字配列を確保せず判定する。差分解析はコアの `patch.rs`、Markdown / ANSI の装飾と描画は GUI 側で行う
    - 差分の構文解析はパス・表示行の種類と本文で再利用する。ハッシュ一致だけでは再利用せず内容も比較する。描画スレッドごとのキャッシュ内容を4 MiB・64項目に制限し、大きすぎる結果は保持しない。表示行のない差分は一つの空結果を共有し、本文を持つ項目を追い出さない。
  - `detail/files.rs` — ファイル一覧・編集履歴の可変高リスト。ファイルの見出し、編集の帯、各編集と展開ボタンを別の項目にし、表示範囲だけ組み立てる。選択全体の編集を保持し、先頭 100 ファイルと編集ごとの 120 行の折りたたみは従来どおり。幅・読み込み世代・ファイル順序が変わっても、パスとイベント位置から上端の編集と行内位置を引き継ぐ
  - `statistics.rs` — 統計画面。見出しに期間とプロジェクトの絞り込み、その下に指標のタイル（値・前の期間との差・読み込み元の割合）。押したタイルの指標で推移・時間帯・プロジェクトを表す。集計中は前の結果を薄く表示する。`statistics/charts.rs` は推移（読み込み元ごとに積んだ列。押すとその期間をタイムラインで開く）と時間帯（曜日 × 時刻の濃淡と周辺の合計）を 1 つの canvas に描き、ポインタの位置で当たり判定する。`statistics/lists.rs` はツール・プロジェクト・モデルの一覧と読み込み元の比較表。配色は Claude = `SERIES_CLAUDE`、Codex = `SERIES_CODEX`、量は青の段階 `HEAT` だけで表す（データ可視化の検査済み） 左右のカードにはグラフと共通の計算済みの幅を指定し、描画ごとの内容からの幅計測を減らす。狭い画面は同じ幅で縦に並べる。
  - `overlay.rs` — ホバーカード・表示オプションのメニュー（表示する値・行とブロックのまとめ方・範囲内のみ。選んでも閉じない）・ショートカット一覧
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

`model/format.rs` は Unicode の文字数による省略と一行表示を提供し、MCP の本文上限にも同じ文字境界を使う。`search/metadata.rs` は詳細分析と MCP の Unicode 語検索を共有する。サイドバー索引の ASCII 小文字化と一致位置の規則は `search.rs` に保つ。

コメントや UI 文言は日本語で書かれている。

### GUI と MCP の内部構成

詳細パネルは `app/detail.rs` の共通枠を中心に、`detail/loading.rs`（取得タスク）、`list.rs`（可変高一覧とアンカー）、`event.rs`（単一イベント）、`history.rs`（対話履歴）、`file_render.rs`（変更ファイルの描画）、`usage.rs`（使用量表示）、`profile.rs`（計測用操作）へ分かれる。既存の `files.rs` はファイル一覧の行モデル、`overview.rs` / `analysis.rs` / `replay.rs` は各機能を担当する。詳細一覧の絞り込み・順序・移動・末尾追従は `detail/controller.rs`、基本情報と再開操作は `detail/overview.rs` が担当する。カレンダーの日・週描画は `app/calendar/day.rs`、月描画は `month.rs` に置く。カレンダーの数値処理は `app/calendar/segments.rs`（区間抽出）と `columns.rs`（重なり列の配置）へ分ける。

MCP の起動・停止は `mcp.rs`、HTTP の境界検査と応答は `mcp/transport.rs`、JSON-RPC は `protocol.rs`、履歴 API の応答・ページング・集計は `tools.rs`、引数検証と一覧・分析で共用するイベントの絞り込みは `query.rs`、一度だけ構築するツール定義は `schema.rs` が管理する。GUI の操作パネルは `app/mcp.rs`。
