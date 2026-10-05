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
cargo clippy                # lint
cargo test                  # GUI を含むテスト
cargo test --lib --no-default-features  # GUI の依存を外してコアだけをテスト
```

`.cargo/config.toml` で linker / CC を `/usr/bin/cc` に固定している（Nix の cc が libiconv を見つけられないため）。変更しないこと。

`RIREKI_CACHE_DIR` と `RIREKI_CONFIG_DIR` でキャッシュと設定の保存先を分けられる。`--profile-ui` は両方を隔離したディレクトリに指定して使うネイティブ画面計測で、コピーした索引だけを読み、ログの差分取り込みを止める。`--profile-detail` を併用すると詳細のスクロール位置・絞り込み・幅変更・タブ・メモリ内の模擬更新を検証する。OS 入力の注入は行わず、公開 API を使って位置と選択を変える。`ZED_MEASUREMENTS=1` で GPUI のフレーム処理の経過時間も記録する。`--bench-ingest` は空のキャッシュを要求し、manifest に固定した安定ログをパース・保存し、本文を含めて再読み込みと比較する。manifest はログのパスを含むため公開しない。`RIREKI_BENCH_LOG_ROOT` を指定すると、その中の `claude` / `codex` ディレクトリを同じ探索処理で読み、隔離したログのコピーで再現できる。

## アーキテクチャ

`src/lib.rs` が GUI に依存しないコアの入口。`model` / `loader` / `db` / `index` / `search` / `pricing` / `jsmock` / `layout` に加え、`statistics` / `detail` / `patch` / `histogram` / `view_index` / `time` を公開する。`src/main.rs` と `app/` はこのライブラリを参照し、GPUI の状態・入力・描画・背景タスクの起動と結果反映を管理する。コアから `app` / `theme` / `icons` / GPUI を参照しない。通常起動は既定の `gui` feature、コアの利用・検証は `--no-default-features`。

- **statistics.rs / statistics/cache.rs** — 統計の期間（7 / 30 / 90 日・全期間）・結果型と、セッションの版を弱参照で識別する集計キャッシュ。直前の同じ長さの期間の合計、読み込み元ごとの列（全期間は 92 日を超えると週、371 日を超えると月ごと）、曜日 × 時刻のマスと曜日・時ごとの合計（セッションは重ねずに数える）、ツール名ごとの呼び出しと失敗も集める。日付と時は各日の午前 0 時の表とレーン内のカーソルで求め、境界の直前が前日・境界が当日にならない日（曖昧な午前 0 時など）があればイベントごとに変換する。テストは全イベントを日時変換する参照実装と比べる。GUI は同じキャッシュを保持し、背景で実行する。
- **detail.rs / detail/summary.rs** — 選択範囲・本文の遅延読み込み・JS キャッシュ・ファイル履歴・選択とセッションの集計。色やアイコンを含めず、名前ごとの先頭イベント位置を返す。GUI はそのイベントの表示を決める。キャンセル確認と本文の表示上限を維持する。
  - 詳細と統計の料金検索は連続するモデルで再利用し、詳細の名前・モデル別集計も直前の位置を再利用する。集計値や料金の加算順序は変えない。
  - 詳細本文は読み取りトランザクションでまとめ、17行以上では64行ずつ問い合わせる。空・単一行・欠損行・重複IDを維持する。選択変更時の古い本文・編集履歴の解放はGUI側で背景へ渡す。
- **patch.rs** — apply_patch / unified diff の解析とファイル・行の構造。描画とシンタックスハイライトは `app/rich.rs` 以下。
- **histogram.rs / time.rs** — イベント開始時刻の区間集計とローカル日付の境界。ミニマップ・カレンダー・統計が共通の規則を使う。
- **view_index.rs** — 非対話セッションの起動元を作業ディレクトリとツールの時刻から求める。`Lane::cum_tool_end` で候補を絞り、長時間の Agent が探索範囲を広げないようにする。DB には保存しない。
- **model.rs / loader.rs / db.rs** — イベント名は `Arc<str>` で共有する。ログのパースはファイルごと、索引の並列読み込みはID範囲ごとに同じ名前を再利用し、メタデータの複製とGPUIの表示でも共有する。DBの値と保存形式は変更しない。
  - 索引のイベントをレーンへ戻すときは、読み取り済みの行から必要数を数えて一度だけ配列を確保する。追加のSQL走査をせず、未使用の容量と再確保時の配列コピーを減らす。
- **text.rs / model::Event::text** — 安定ログの保存待ちの本文はファイルごとの圧縮データを共有し、成功したコミット後にDBの行IDへ置き換える。同じ版のスナップショットは保存状態を共有し、コミット後に圧縮本文を解放する。今回書き込んだファイルを更新してDB行を削除する前に、まだ参照されている旧版の本文だけを圧縮して退避する。書き込み接続では削除済みのイベントIDを再利用せず、更新前に取得したIDと新しい本文の取り違えを防ぐ。リアルタイムの本文は通常のString、保存失敗時は圧縮本文から詳細を開く。イベント本文を取得するときは `Event::text()` を使用する。`Event::text_ref` は行IDと保存状態を一つの領域に収め、通常のDB読み込みでもイベントのサイズを増やさない。
- **index.rs** — 保存待ちの指紋は保存済みと区別し、成功した書き込みだけを保存済みにする。失敗時は次の探索で再試行し、古い保存完了が新しい版やリアルタイムの指紋を書き換えない。安定ログは並列ワーカー内でファイルごとにパース・圧縮・本文解放まで連続して行い、未圧縮本文の一時保持を作業中のファイルに限定する。画面反映とDB書き込みは64ファイル単位を維持する。
- **search.rs** — 初回の検索索引の長さ計算・連結・小文字化は256イベントごとに中断を確認する。中断した途中の索引は公開せず、完成したセッションの版だけを再利用する。
- **perf.rs / perf/** — コアの公開 API を直接使う CLI の計測。画面用のモジュールを経由しない。取り込みの比較用の全件パースと内容照合は取り込み時間の外で行い、Storeの共有された版からも保存後の本文とイベントの対応を検証する。

起動計測は `--profile-ui --profile-startup`。`main_to_ready_ms` は main の開始から、データを含むフレームの描画・提示後、次のフレーム要求までの時間。OS のローダーと GPU 完了時間は含まない。`--profile-ingest manifest.json` を加えると空の隔離キャッシュから通常の取り込み経路を動かす。`--profile-idle claude/projects codex/sessions 15` は指定したルートを 3 秒ごとに探索し、指定秒数待つ。全ログが索引済みでなければパース前に停止する。`--profile-rich` は差分展開・単一長文・24 時間の日週表示を検証する。いずれも隔離したキャッシュと設定を指定する。

`--profile-ui --profile-files` はファイル履歴の展開・幅変更・並びの更新で同じ編集行を保ち、その編集を開くことを検証する。画面ロックで実画面のフレーム要求が止まる場合、`--profile-render` を併用して公開 API の `Window::draw` と要素の解放を測れる。この値は描画命令の構築・レイアウト・paint を含み、GPU への提示は含まない。`--profile-eager-files` は計測専用で、同じ行構造をすべて組み立てる比較用の方式に切り替える。通常のファイル一覧は可視行だけを作る。

`--profile-ui --profile-render --profile-summary` は大きな選択範囲とセッションのタブを幅変更しながら繰り返し描画する。詳細の集計・本文コピー・ユーザー指示の区間索引は背景で作り、セッションの集計は最後の一つの版だけを弱参照で識別して再利用する。集計途中は 256 イベントごとに中断し、表示には最新の選択と版に対応する結果だけを反映する。

データの流れ: 背景の `index::ViewLoader` が索引 DB の読み込み、`Indexer::plan` / `process`、`model::Store::replace` と描画用の索引化を行う。画面と検索は `model::Snapshot`（`Arc<Session>` の集合）を受け取り、更新前の版は読み終わるまで保つ。3 秒ごとにリアルタイムファイルを再読込する。

- **index.rs** — ファイルを「安定」（最終更新から `LIVE_SECS` 以上経過）と「リアルタイム」に分ける。安定ファイルはパース結果を DB に保存し、指紋（mtime + サイズ）が変わるまで読み直さない。リアルタイムファイルは DB に書かずメモリ上で毎回パースし、更新が止まると安定側へ移る。
- **db.rs** — 索引 DB (`~/Library/Caches/rireki/index.db`, SQLite/rusqlite)。`files` / `items` / `lanes` / `events` / `texts`。起動時は `events` だけを ID 範囲で分割して並列に読み、大きな input / output (`texts`) は詳細表示のときに `Event::row()` で引く。`texts` は lz4 圧縮して別ファイル `texts.db` に置く（同じファイルだと `events` のページが散らばり起動が遅くなる）。書き込みは `Indexer` の専用スレッドで行い、次のチャンクのパースと重ねる。パース結果やスキーマを変えたら `VERSION` を上げる（DB が作り直される）。
- **loader.rs** — 読み込み元は `~/.claude/projects/*/*.jsonl` と `$CODEX_HOME/sessions`（既定 `~/.codex/sessions`、再帰探索）。ファイルは mtime の新しい順に `Job` 化される。
  - JSONのYAML風表示は行を順に生成し、6,000文字の一文字先で止めて従来と同じ位置に省略記号を付ける。長い文字列・キーも必要な先頭だけを複製する。配列の1行表示は80文字を超えると判定を止め、全文を連結しない。日本語・字下げ・空の値・改行の扱いを維持する。
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
  - `range.rs` — 期間を選ぶカレンダー（ポップオーバー、日ごとのイベント量を棒で表示）と、タイムライン上端のミニマップ（選んだ期間、未選択なら全期間のイベント分布と現在の表示範囲。クリック・ドラッグで移動）
  - `detail.rs` — 詳細パネル（`DetailView`）。選択している間だけ開く。見出しに結果の状態・レーン内の位置と前後のイベントへの移動（J / K）。その下の文脈の帯に「読み込み元 › プロジェクト › セッション › サブエージェント」のパンくず（セッション名でセッションのタブへ、矢印で起動元へ）。その下のタブ（`prefs::DetailTab`、1 / 2 / 3）で「イベント」「ファイル」（編集したファイルと追加・削除の割合。押すとその場で開き、編集の時系列の帯と編集ごとの差分を出す）「セッション」（集計のタイル・操作・サブエージェントの一覧・トークンと料金・属性）を切り替える。イベントのタブは 1 イベントなら時刻・トークン・きっかけの指示と入力・結果、複数なら集計のタイル（失敗は押すと絞り込み）と、名前・失敗で絞り込めるイベント一覧（ユーザーの指示ごとに区切る）。本文は選択時に `request_detail` で背景処理へ渡し、コアの `detail::DetailLoader` が DB 読み込み・JS の整形を行って `DetailCache` に持つ。古い選択の結果は世代で破棄する。複数イベントの一覧は `DetailList` の可変高リストで可視行だけ作り、幅変更や更新では上端のイベントと行内位置を保つ。先頭 300 件に絞り込みを掛ける順序は変えない。末尾にいた場合と追従アニメーション中は連続更新を追い、ユーザーのスクロールや絞り込み・タブ変更で追従を中断する
  - `rich.rs` — 詳細パネルの本文の表示: Markdown（メッセージ・計画・サブエージェントの報告）、行番号付きの差分、ANSI の色、`cat -n` 形式の行番号、YAML 風の引数のキーの色分け。差分解析はコアの `patch.rs`、Markdown / ANSI の装飾と描画は GUI 側で行う
    - 差分の構文解析はパス・表示行の種類と本文で再利用する。ハッシュ一致だけでは再利用せず内容も比較する。描画スレッドごとのキャッシュ内容を4 MiB・64項目に制限し、大きすぎる結果は保持しない。表示行のない差分は一つの空結果を共有し、本文を持つ項目を追い出さない。
  - `detail/files.rs` — ファイル一覧・編集履歴の可変高リスト。ファイルの見出し、編集の帯、各編集と展開ボタンを別の項目にし、表示範囲だけ組み立てる。選択全体の編集を保持し、先頭 100 ファイルと編集ごとの 120 行の折りたたみは従来どおり。幅・読み込み世代・ファイル順序が変わっても、パスとイベント位置から上端の編集と行内位置を引き継ぐ
  - `statistics.rs` — 統計画面。見出しに期間とプロジェクトの絞り込み、その下に指標のタイル（値・前の期間との差・読み込み元の割合）。押したタイルの指標で推移・時間帯・プロジェクトを表す。集計中は前の結果を薄く表示する。`statistics/charts.rs` は推移（読み込み元ごとに積んだ列。押すとその期間をタイムラインで開く）と時間帯（曜日 × 時刻の濃淡と周辺の合計）を 1 つの canvas に描き、ポインタの位置で当たり判定する。`statistics/lists.rs` はツール・プロジェクト・モデルの一覧と読み込み元の比較表。配色は Claude = `SERIES_CLAUDE`、Codex = `SERIES_CODEX`、量は青の段階 `HEAT` だけで表す（データ可視化の検査済み）
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
