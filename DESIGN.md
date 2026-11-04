---
version: alpha
name: rireki
description: Claude Code と Codex のセッションログを読む、暗い面のデスクトップアプリ。色はデータと選択状態だけに使い、面と罫線で階層を作る。

colors:
  # 面。奥から手前へ backdrop → bg → panel → raised → hover の順に明るくなる。
  backdrop: "#0d0f13"
  bg: "#16181d"
  panel: "#1f2227"
  raised: "#282b31"
  hover: "#32353c"
  line: "#3a3d44"
  line-soft: "#282b31"
  # 文字。tokens.ts / theme.rs の text・muted・subtle。
  text: "#d8dbe1"
  muted: "#a3a8b1"
  subtle: "#8b8f98"
  # 操作と選択。tokens.ts / theme.rs の accent・accentBg にあたる。
  primary: "#5b9cf5"
  primary-bg: "#22334f"
  select: "#2a5aa8"
  # 状態。
  live: "#3fb950"
  now: "#d8dbe1"
  err: "#f07c7c"
  warn: "#e0a24a"
  diff-add: "#7fd18b"
  diff-del: "#f07c7c"
  # 読み込み元の系列色（グラフ・統計）とロゴの地色。
  claude: "#d95926"
  codex: "#199e70"
  logo-claude: "#d97757"
  logo-codex: "#d0d0d0"
  # 幕と影。ポップオーバーとモーダルだけに使う。
  scrim: "#00000073"
  shadow: "#0008"
  # 量を表す青の段階（少 → 多）。
  heat-1: "#184f95"
  heat-2: "#256abf"
  heat-3: "#3987e5"
  heat-4: "#6da7ec"
  heat-5: "#9ec5f4"
  # イベントの色。tokens.ts の eventGroups。
  event-user: "#aa8d00"
  event-assistant: "#b3b7c1"
  event-thinking: "#565a62"
  event-reference: "#00719e"
  event-edit: "#477200"
  event-shell: "#967cdb"
  event-agent: "#953d7c"
  event-other: "#82868f"
  # プロジェクトの目印の色。
  project-1: "#5d7fb0"
  project-2: "#5e9a82"
  project-3: "#b08f55"
  project-4: "#a66f97"
  project-5: "#b8714f"
  project-6: "#4f97a6"
  project-7: "#949a52"
  project-8: "#b35f70"

typography:
  sans:
    fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans JP', sans-serif"
    fontWeight: 400
    lineHeight: 1.45
  mono:
    fontFamily: "'SFMono-Regular', Consolas, 'Liberation Mono', monospace"
    fontWeight: 400
  axis:
    fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans JP', sans-serif"
    fontSize: 11px
    fontWeight: 400
    lineHeight: 1.45
  axis-strong:
    fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans JP', sans-serif"
    fontSize: 11px
    fontWeight: 600
    lineHeight: 1.45
  small:
    fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans JP', sans-serif"
    fontSize: 12px
    fontWeight: 400
    lineHeight: 1.45
  small-strong:
    fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans JP', sans-serif"
    fontSize: 12px
    fontWeight: 600
    lineHeight: 1.45
  body:
    fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans JP', sans-serif"
    fontSize: 13px
    fontWeight: 400
    lineHeight: 1.45
  heading:
    fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans JP', sans-serif"
    fontSize: 16px
    fontWeight: 600
    lineHeight: 1.45
  value:
    fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans JP', sans-serif"
    fontSize: 20px
    fontWeight: 600
    lineHeight: 28px
    fontFeature: "'tnum'"

rounded:
  sm: 3px
  md: 6px
  panel: 10px
  inner: 9px
  full: 9999px

spacing:
  # 余白の段階。
  "1": 4px
  "2": 8px
  "3": 12px
  "4": 16px
  "5": 20px
  "6": 24px
  # 画面の骨格の寸法。
  strip: 34px
  rail: 68px
  gap: 6px
  header: 46px
  footer: 26px
  minimap: 30px
  axis: 40px
  row: 32px
  sidebar: 260px
  label: 260px
  detail: 440px
  # ボタン・入力・行に共通する操作部品の高さ。
  control: 28px

# 透明度は意味ごとに 1 つ。これ以外の透明度で色を作らない。
alpha:
  disabled: 0.4
  stale: 0.55
  range: 0.2
  diff: 0.12

components:
  workspace:
    backgroundColor: "{colors.bg}"
    rounded: "{rounded.panel}"
  sidebar:
    backgroundColor: "{colors.bg}"
    width: "{spacing.sidebar}"
  detail-panel:
    backgroundColor: "{colors.bg}"
    width: "{spacing.detail}"
  rail:
    backgroundColor: "{colors.backdrop}"
    textColor: "{colors.muted}"
    width: "{spacing.rail}"
  title-strip:
    textColor: "{colors.subtle}"
    typography: "{typography.axis}"
    height: "{spacing.strip}"
  toolbar:
    backgroundColor: "{colors.bg}"
    height: "{spacing.header}"
  minimap:
    backgroundColor: "{colors.bg}"
    height: "{spacing.minimap}"
  divider:
    backgroundColor: "{colors.line}"
    height: 1px
  footer:
    backgroundColor: "{colors.bg}"
    textColor: "{colors.muted}"
    typography: "{typography.axis}"
    height: "{spacing.footer}"

  button:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.text}"
    typography: "{typography.small}"
    rounded: "{rounded.md}"
    padding: "{spacing.2}"
    height: "{spacing.control}"
  button-hover:
    backgroundColor: "{colors.hover}"
    textColor: "{colors.text}"
  button-primary:
    backgroundColor: "{colors.select}"
    textColor: "{colors.text}"
  button-danger:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.err}"
  icon-button:
    textColor: "{colors.muted}"
    rounded: "{rounded.md}"
    size: 26px
  icon-button-hover:
    backgroundColor: "{colors.hover}"
    textColor: "{colors.text}"
  icon-button-active:
    backgroundColor: "{colors.primary-bg}"
    textColor: "{colors.primary}"
  rail-button:
    textColor: "{colors.muted}"
    rounded: "{rounded.inner}"
    size: 36px
  rail-button-hover:
    backgroundColor: "{colors.raised}"
    textColor: "{colors.text}"
  rail-button-active:
    backgroundColor: "{colors.hover}"
    textColor: "{colors.text}"

  segmented:
    backgroundColor: "{colors.bg}"
    rounded: "{rounded.md}"
    padding: "{spacing.1}"
    height: "{spacing.control}"
  segmented-item:
    textColor: "{colors.muted}"
    typography: "{typography.small}"
    rounded: "{rounded.sm}"
    padding: "{spacing.2}"
  segmented-item-hover:
    backgroundColor: "{colors.hover}"
    textColor: "{colors.text}"
  segmented-item-active:
    backgroundColor: "{colors.primary-bg}"
    textColor: "{colors.text}"
    typography: "{typography.small-strong}"
  focus-chip:
    backgroundColor: "{colors.primary-bg}"
    textColor: "{colors.primary}"
    typography: "{typography.small}"
    rounded: "{rounded.md}"
  search-input:
    backgroundColor: "{colors.backdrop}"
    textColor: "{colors.muted}"
    typography: "{typography.small}"
    rounded: "{rounded.md}"
    padding: "{spacing.2}"
    height: 30px
  search-input-focus:
    textColor: "{colors.primary}"
  search-match:
    backgroundColor: "{colors.primary-bg}"
    textColor: "{colors.text}"
    rounded: "{rounded.sm}"
  kbd:
    backgroundColor: "{colors.bg}"
    textColor: "{colors.muted}"
    typography: "{typography.axis}"
    rounded: "{rounded.sm}"

  sidebar-row:
    textColor: "{colors.text}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: "{spacing.2}"
    height: "{spacing.control}"
  sidebar-row-hover:
    backgroundColor: "{colors.raised}"
  sidebar-row-selected:
    backgroundColor: "{colors.select}"
    textColor: "{colors.text}"
  sidebar-row-meta:
    textColor: "{colors.subtle}"
    typography: "{typography.axis}"
  section-heading:
    textColor: "{colors.muted}"
    typography: "{typography.axis-strong}"
    height: 22px

  event-block:
    backgroundColor: "{colors.raised}"
    textColor: "{colors.text}"
    typography: "{typography.axis}"
    rounded: "{rounded.sm}"
    height: 20px
  event-block-hover:
    backgroundColor: "{colors.hover}"
  event-block-selected:
    backgroundColor: "{colors.hover}"
    textColor: "{colors.primary}"
  event-block-failed:
    textColor: "{colors.err}"
  event-block-rejected:
    textColor: "{colors.warn}"
  now-line:
    backgroundColor: "{colors.now}"
    width: 1px
  range-selection:
    backgroundColor: "{colors.primary}"
    rounded: "{rounded.sm}"

  legend-dot-user:
    backgroundColor: "{colors.event-user}"
    rounded: "{rounded.full}"
    size: 8px
  legend-dot-assistant:
    backgroundColor: "{colors.event-assistant}"
    rounded: "{rounded.full}"
    size: 8px
  legend-dot-thinking:
    backgroundColor: "{colors.event-thinking}"
    rounded: "{rounded.full}"
    size: 8px
  legend-dot-reference:
    backgroundColor: "{colors.event-reference}"
    rounded: "{rounded.full}"
    size: 8px
  legend-dot-edit:
    backgroundColor: "{colors.event-edit}"
    rounded: "{rounded.full}"
    size: 8px
  legend-dot-shell:
    backgroundColor: "{colors.event-shell}"
    rounded: "{rounded.full}"
    size: 8px
  legend-dot-agent:
    backgroundColor: "{colors.event-agent}"
    rounded: "{rounded.full}"
    size: 8px
  legend-dot-other:
    backgroundColor: "{colors.event-other}"
    rounded: "{rounded.full}"
    size: 8px
  live-dot:
    backgroundColor: "{colors.live}"
    rounded: "{rounded.full}"
    size: 7px
  source-bar-claude:
    backgroundColor: "{colors.claude}"
    rounded: "{rounded.sm}"
  source-bar-codex:
    backgroundColor: "{colors.codex}"
    rounded: "{rounded.sm}"
  source-logo-claude:
    backgroundColor: "{colors.logo-claude}"
  source-logo-codex:
    backgroundColor: "{colors.logo-codex}"
  status-warn:
    textColor: "{colors.warn}"
    typography: "{typography.small}"
  status-err:
    textColor: "{colors.err}"
    typography: "{typography.small}"
  diff-line-add:
    textColor: "{colors.diff-add}"
    typography: "{typography.mono}"
  diff-line-del:
    textColor: "{colors.diff-del}"
    typography: "{typography.mono}"
  metric-value:
    textColor: "{colors.text}"
    typography: "{typography.value}"
  project-dot-1:
    backgroundColor: "{colors.project-1}"
    rounded: "{rounded.full}"
    size: 8px
  project-dot-2:
    backgroundColor: "{colors.project-2}"
    rounded: "{rounded.full}"
    size: 8px
  project-dot-3:
    backgroundColor: "{colors.project-3}"
    rounded: "{rounded.full}"
    size: 8px
  project-dot-4:
    backgroundColor: "{colors.project-4}"
    rounded: "{rounded.full}"
    size: 8px
  project-dot-5:
    backgroundColor: "{colors.project-5}"
    rounded: "{rounded.full}"
    size: 8px
  project-dot-6:
    backgroundColor: "{colors.project-6}"
    rounded: "{rounded.full}"
    size: 8px
  project-dot-7:
    backgroundColor: "{colors.project-7}"
    rounded: "{rounded.full}"
    size: 8px
  project-dot-8:
    backgroundColor: "{colors.project-8}"
    rounded: "{rounded.full}"
    size: 8px
  heat-cell-1:
    backgroundColor: "{colors.heat-1}"
  heat-cell-2:
    backgroundColor: "{colors.heat-2}"
  heat-cell-3:
    backgroundColor: "{colors.heat-3}"
  heat-cell-4:
    backgroundColor: "{colors.heat-4}"
  heat-cell-5:
    backgroundColor: "{colors.heat-5}"

  tooltip:
    backgroundColor: "{colors.raised}"
    textColor: "{colors.text}"
    typography: "{typography.small}"
    rounded: "{rounded.md}"
    padding: "{spacing.2}"
  popover:
    backgroundColor: "{colors.raised}"
    textColor: "{colors.text}"
    typography: "{typography.small}"
    rounded: "{rounded.md}"
    padding: "{spacing.3}"
  modal:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.text}"
    rounded: "{rounded.panel}"
    width: 660px
  modal-heading:
    textColor: "{colors.text}"
    typography: "{typography.heading}"
    padding: "{spacing.5}"
  modal-backdrop:
    backgroundColor: "{colors.scrim}"
  empty-state:
    textColor: "{colors.muted}"
    typography: "{typography.body}"
    padding: "{spacing.6}"
---

## Overview

rireki は Claude Code と Codex のセッションログを、タイムライン・カレンダー・統計・詳細として読む道具です。画面は情報の密度が高く、長く眺めても疲れない暗い面で組みます。判断の基準は一つで、迷ったら「これは情報か、飾りか」を問い、飾りは足しません。

画面は Slack に倣い、上端の帯 `title-strip`、左端の `rail`、`sidebar`、メイン列の `workspace`、右の `detail-panel` を並べます。メイン列はツールバー `toolbar`、タイムライン上端の `minimap`、本体、下端の `footer` の順に積みます。

色・面・角丸・文字・余白・アイコン・文言・状態の原則は次の各節にまとめます。値は frontmatter のトークンだけが持ち、本文は名前で参照します。実装との対応は次のとおりです。

- `design/src/tokens.ts` が値の正本で、`src/theme.rs` と `design/src/styles.css` が名前を揃えて追従します。この `DESIGN.md` はその要約で、`tokens.ts` を変えたら合わせます。
- `primary` と `primary-bg` は、`tokens.ts` と `theme.rs` では `accent` と `accent_bg` と呼びます。`logo-claude` と `logo-codex` は `brand` の二つ、`event-*` は `eventGroups`、`project-*` は `projectColors`、`heat-*` は `heat` にあたります。

## Colors

色を使うのはデータと選択状態だけです。データとは、イベントの種類、Claude と Codex、プロジェクト、失敗・拒否・中断です。成功は無色にします。文字は文字色だけで書き、データの色は隣に置く印（点や線）で示します。

**面と罫線**: 面は `backdrop`、`bg`、`panel`、`raised`、`hover` の 5 段で、奥から手前へ明るくなります。`backdrop` はウィンドウの地、`bg` は内容パネル・サイドバー・詳細、`panel` はボタンやモーダルの面、`raised` は浮かせる面（ポップオーバー、ツールチップ、イベントブロック）、`hover` はポインターが乗った面です。区切りは `line` の罫線 1 本、弱い区切りは `line-soft` を使います。グレーは明度だけで動かし、色相と彩度は固定します。

**文字**: `text` が本文、`muted` が補助、`subtle` が最も弱い文字です。`subtle` は `panel` の上でのみ読める明るさなので、`raised` と `hover` の上では使いません。

**操作と選択**: `primary` は操作の強調とフォーカス、`primary-bg` はその下地（有効なアイコンボタン、一致箇所、選択中のセグメント）です。行の選択は `select` で塗り、文字は `text` のままにします。

**状態**: 進行中は `live`、現在時刻の線は `now`、失敗は `err`、拒否・中断は `warn`、差分の追加と削除は `diff-add` と `diff-del` です。

**読み込み元**: Claude は `claude`、Codex は `codex` を系列色にします。ロゴの地色の `logo-claude` と `logo-codex` はブランドの色なので、ロゴ以外には使いません。

**量**: 統計の濃淡と使用量の内訳には、青の 5 段 `heat-1` から `heat-5` だけを使います。数が増えるほど段が上がります。

**イベントの色**: 判別できる色相は 5 つ（ユーザー・参照・編集・シェル・エージェント）に絞り、応答・思考・その他は無彩色の濃淡にします。色相を増やすと全ペアの識別が保てなくなるので増やしません。色を足すときは、配色検証器を全ペアで通してから `tokens.ts` と `styles.css` に追加します。`claude`、`codex`、`err` はイベントに使いません。ブロックの色だけに頼らず、アイコンとラベル、凡例で補います。

**プロジェクト**: `project-1` から `project-8` は目印です。必ず名前を並べるので、同じ色になっても構いません。

**透明度**: `alpha` の 4 つだけを使い、意味は名前のとおりです。`disabled` は無効、`stale` は再集計中の古い結果、`range` は範囲の塗り、`diff` は差分の行です。色に透明度を掛けた塗りでバッジを作りません。幕 `scrim` と影 `shadow` は透明度を含む色として別に持ちます。

## Typography

文字は 5 段で、名前は小さい順に `axis`、`small`、`body`、`heading`、`value` です。太さは通常と強調の 2 つだけで、強調は `axis-strong` と `small-strong` のように `-strong` を付けた名前で表します。

- `axis`: 軸、時刻、件数、キー表記 `kbd`、セクション見出し、フッターなどの補助の文字。
- `small`: ボタン、セグメント、検索入力、ツールチップ、タブ、状態の文字。
- `body`: サイドバーの行、空の状態。
- `heading`: 詳細パネルの文脈タイトル、モーダルの見出し。
- `value`: 統計の指標の値。

数値は等幅数字にそろえます（`value` の `fontFeature`）。表や順位の数値も同じ扱いです。`mono` はコード、差分、MCP の接続設定にだけ使います。本文のフォントは Storybook では `sans` の並びを使い、GPUI 本体は既定のフォントに任せています。

## Layout

画面の骨格の寸法は `spacing` の名前付きの項目が持ちます。上端の帯は `strip`、レールは `rail`、ツールバーは `header`、ミニマップは `minimap`、フッターは `footer`、サイドバーは `sidebar`、詳細パネルは `detail`、タイムラインのラベル列は `label`、軸は `axis`、行は `row` です。部品どうしの隙間は `gap` です。ボタン・入力・サイドバーの行の高さは `control` にそろえます。

余白は `spacing` の `1` から `6` の段階だけで、負のマージンは使いません。例外は、グラフの棒や点の間にある細い区切りのすき間です。

上端の帯はウィンドウの移動とズームに使う領域なので、操作する部品を置きません。内容パネルは角丸なので、角に接する子要素は背景を塗らないか、`inner` の角丸にそろえます。

区切りは余白か罫線 1 本（`divider`）で作ります。枠と角丸で囲むカードは作りません。

## Elevation & Depth

階層は面の段（`bg` → `panel` → `raised` → `hover`）と罫線 `line` で表し、影は使いません。例外はポップオーバーとモーダルで、この 2 つだけが `shadow` を持ちます。

幕 `scrim` はモーダルの背後にだけ敷きます。ポップオーバーには幕がなく、外側をクリックすると閉じます。重なりの順は、イベントブロックの hover、ペインの境界、ポップオーバーの外側、ポップオーバー、モーダルの幕の順に上がります。

色付きの左帯は付けません。

## Shapes

角丸は `rounded` の `sm`、`md`、`panel`、`inner` の 4 段で、正円の `full` は点（凡例、進行中、プロジェクトの目印）にだけ使います。`sm` はイベントブロックや一致箇所などの小さい部品、`md` はボタン・入力・行・ポップオーバー、`panel` は内容パネルとモーダルです。`inner` は `panel` の枠線の内側に収まる子要素に使い、枠線を覆わないようにします。

## Components

部品の名前と状態は frontmatter の `components` にあります。状態は `-hover`、`-active`、`-selected`、`-focus` を付けた別の項目で表し、ここでは使い分けの方針だけを書きます。

- **ボタン**: 通常の `button`、強調の `button-primary`、取り消せない操作の `button-danger`。アイコンだけのボタンは `icon-button` で、有効のときは `icon-button-active` の組み合わせにします。レールのボタンは `rail-button` です。
- **切り替え**: 表示モードは `segmented` と `segmented-item`。選択中は `segmented-item-active` で、文字を強調します。絞り込みの表示には `focus-chip` を使います。
- **入力と一覧**: 検索は `search-input`、一致箇所は `search-match`。サイドバーの行は `sidebar-row`、選択は `sidebar-row-selected`、補助の値は `sidebar-row-meta`、グループの見出しは `section-heading` です。
- **タイムライン**: `event-block` は種類を問わず同じ面で、種類はアイコンの色で示します。選択は `event-block-selected`、失敗は `event-block-failed`、拒否と中断は `event-block-rejected` です。現在時刻は `now-line`、選んだ範囲は `range-selection` で示します。
- **データの印**: 凡例は `legend-dot-*`、進行中は `live-dot`、プロジェクトは `project-dot-*`、量は `heat-cell-*`、読み込み元の系列は `source-bar-*`、ロゴは `source-logo-*` です。
- **重なる面**: `tooltip`、`popover`、`modal`。モーダルは見出し `modal-heading` と背後の `modal-backdrop` を持ちます。
- **状態の表示**: 空の状態は `empty-state` の 1 行の文字だけです。再集計中は前の結果を `alpha` の `stale` で薄く表示し、前の結果がない初回は「読み込み中…」の 1 行だけを出します。

アイコンは Lucide を使い、文字だけでは区別できない場所に限ります。イベントの種類のアイコンは種類ごとに決まっていて、色は種類の属するグループに従います。

## Do's and Don'ts

**やること**

- 値は `tokens.ts` のトークンだけから選ぶ。新しい色・寸法・透明度が必要なら、先にトークンを足す。
- 色を使う前に、それがデータか選択状態かを確かめる。
- 区切りは余白か罫線 1 本にする。
- 同じ概念は 1 語で呼ぶ（料金は「料金」）。
- 画面から読めない事実だけを、該当する値のツールチップに 1 文で書く。アイコンだけのボタンは、同じ文言を `aria-label` と `title` に一度ずつ付ける。
- 新しいツール名を扱うときは、色（カテゴリ単位）とアイコン（ツール単位）を合わせて足す。

**やらないこと**

- 色に透明度を掛けたバッジを作らない。`alpha` にない透明度で色を作らない。
- 枠と角丸で囲むカード、色付きの左帯、ポップオーバーとモーダル以外の影を作らない。
- `rounded` にない角丸、`typography` にない文字サイズ、`spacing` にない余白を使わない。正円を点以外に使わない。
- 見出しや飾りのためのアイコン、全行に同じアイコン、飾りの枠、意味の合わないアイコンを付けない。
- 見れば分かる補足、脚注、2 段目の説明を書かない。空状態に説明文や操作を足さない。
- `subtle` を `raised` や `hover` の上に置かない。
- 成功に色を付けない。`claude`、`codex`、`err` をイベントの色に使わない。
- 同じ要素に、別々の文言の `aria-label` と `title` を付けない。
