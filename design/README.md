# rireki UI / Storybook

Rust / GPUI で実装された現在の GUI をブラウザーで確認するための Storybook です。
React / TypeScript / Vite を使い、本体と独立して起動・ビルドできます。

## 起動

Node.js 22.12 以降、npm 10 以降を使用してください。

```sh
cd design
npm ci
npm run storybook
```

http://127.0.0.1:6007 で開きます。別のポートを使う場合は
`npm run storybook -- --port 6010` を実行してください。

## 収録内容

- **画面 / タイムライン**: 全履歴、読み込み元・プロジェクトの絞り込み、API コスト、ディレクトリ表示、検索、サイドバーの開閉。
- **画面 / カレンダー**: 日・週・月、セッション選択。
- **画面 / 統計**: 6 指標、推移、プロジェクト・セッション順位、曜日と時間帯、モデル、ツール、読み込み元。
- **画面 / 状態**: 読み込み中、履歴なし、検索結果なし。
- **パネル / セッション詳細**: 概要、履歴、ツールの入出力・失敗、変更と差分、分析、全画面、進行中。
- **パネル / オーバーレイ**: 表示設定、期間・日付の選択、キー操作、MCP の停止中・実行中。
- **コンポーネント / 基本部品**: ボタン、イベントブロック、状態表示、集計、セグメント切り替え。
- **デザイントークン / テーマ**: 配色、イベントの色、寸法。

「操作：」で始まるストーリーは `play` 関数で操作を実行し、結果を検証します。
Controls で初期状態を変えるとコンポーネントを作り直すため、再現したい状態を安定して確認できます。

## 操作と再現範囲

読み込み元・プロジェクト・検索語による絞り込み、セッション・イベント選択、詳細タブ、
カレンダー切り替え、期間移動・拡大縮小、ブロック集約、パネルの幅変更、
履歴の再生、分析フィルター、コピー、サンプルイベントの JSON 保存を操作できます。
パネルの境界はドラッグ、またはフォーカスして左右キーで変更できます。
検索欄には `⌘K` / `Ctrl+K`、詳細タブには `1`–`4`、各表示には `D` / `W` / `M` を使えます。

データと現在時刻は **2026/10/26 の架空サンプル**に固定しています。
統計は共通サンプルから算出し、日・週・月とタイムラインで同じセッションを使います。
入力・結果、差分、使用量の内訳は UI 確認用のサンプルです。
実際の JSONL ログや本体のキャッシュは読み込みません。
MCP の開始・停止は表示状態のシミュレーションで、サーバーを起動しません。
再開コマンドのコピーにもサンプルのセッション ID を使用します。

ネイティブ GUI の既定サイズ 1500 × 920、最小サイズ 960 × 600 を基準としたデスクトップ UI です。
画面の高さはプレビューに追従します。狭い画面では Storybook の全画面表示または新しいタブで確認してください。
GPUI の描画エンジンや OS のフォントラスタライズまでは共有していません。
TUI は対象外です。

## 実装との対応

| Storybook                          | 参照する本体の実装                                                       |
| ---------------------------------- | ------------------------------------------------------------------------ |
| `src/tokens.ts` / `src/styles.css` | `src/theme.rs`、`src/app.rs`、`src/prefs.rs`                             |
| `src/RirekiApp.tsx`                | `src/app.rs`、`src/app/header.rs`、`src/app/rail.rs`、`src/app/panes.rs` |
| `src/components/Sidebar.tsx`       | `src/app/sidebar.rs`                                                     |
| `src/components/Timeline.tsx`      | `src/app/timeline.rs`、`src/app/cost.rs`                                 |
| `src/components/Calendar.tsx`      | `src/app/calendar/day.rs`、`src/app/calendar/month.rs`                   |
| `src/components/DetailPanel.tsx`   | `src/app/detail.rs`、`src/app/detail/`                                   |
| `src/components/Statistics.tsx`    | `src/app/statistics.rs`、`src/app/statistics/`                           |
| `src/components/Overlays.tsx`      | `src/app/overlay.rs`、`src/app/range.rs`、`src/app/mcp.rs`               |
| `src/components/primitives.tsx`    | `src/app/widgets.rs`、`src/icons.rs`                                     |

Claude / Codex / rireki のロゴはリポジトリの `assets/` を直接参照します。
一般アイコンは本体と同じ Lucide です。色・寸法・日本語ラベルを本体で変更した際は、対応する Storybook も更新してください。

## 検証と静的ビルド

```sh
npm run typecheck
npm run format:check
npx playwright install chromium  # 初回のブラウザーテスト用
npm test
npm run build-storybook
```

`npm test` は Chromium 上で全ストーリーを描画し、操作ストーリーのアサーションを検証します。
`npm run test:watch` または Storybook 左下のテスト UI でも実行できます。
アクセシビリティアドオンは、既存の配色を保持した状態で問題を確認するためのレポートとして有効にしています。
既存 UI と同じコントラストの箇所は自動テストの失敗条件にはしていません。

静的ファイルは `design/storybook-static/` に生成されます。
`node_modules/`、ビルド成果物、テスト成果物は Git に含めません。
