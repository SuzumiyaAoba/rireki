// デザイントークンの正。名前は src/theme.rs（UPPER_SNAKE）と styles.css の :root（--kebab）に揃える。
// 本体 src/theme.rs は Stage 2 でこの値に追従し、テストで一致を検査する。
// グレーは OKLCH の色相 265・彩度 0.014 以下で明度だけを動かす。
export const colors = {
  backdrop: '#0d0f13',
  bg: '#16181d',
  panel: '#1f2227',
  raised: '#282b31',
  hover: '#32353c',
  line: '#3a3d44',
  lineSoft: '#282b31',
  text: '#d8dbe1',
  muted: '#a3a8b1',
  subtle: '#8b8f98', // PANEL 上で 4.5:1 以上。RAISED・HOVER の上では使わない
  accent: '#5b9cf5',
  accentBg: '#22334f',
  select: '#2a5aa8',
  live: '#3fb950',
  now: '#d8dbe1',
  err: '#f07c7c',
  warn: '#e0a24a',
  diffAdd: '#7fd18b',
  diffDel: '#f07c7c',
  claude: '#d95926',
  codex: '#199e70',
} as const;

export const dimensions = {
  strip: 34,
  rail: 68,
  gap: 8,
  header: 46,
  footer: 26,
  minimap: 30,
  axis: 40,
  row: 32,
  control: 28,
  sidebar: 260,
  label: 260,
  detail: 440,
} as const;

// 角丸は 4 段。正円の full は点と目印（今日の日付・履歴のノード・ドーナツの穴）だけに使う。
export const radius = { sm: 3, md: 6, panel: 10, inner: 9, full: 9999 } as const;

// 文字サイズは 5 段。11px は軸・時刻・件数・kbd などの補助の文字。太さは通常と 600 の 2 つ。
export const fontSize = { axis: 11, small: 12, body: 13, heading: 16, value: 20 } as const;

// 余白は 4px の倍数だけ。
export const space = { 1: 4, 2: 8, 3: 12, 4: 16, 5: 20, 6: 24 } as const;

// 透明度は意味ごとに 1 つ（無効・古い結果・範囲の塗り・差分の行）。これ以外の透明度で色を作らない。
export const alpha = { disabled: 0.4, stale: 0.55, range: 0.2, diff: 0.12 } as const;

// 量を表す青の段階（少 → 多）。統計の濃淡と使用量の内訳に使う。
export const heat = ['#184f95', '#256abf', '#3987e5', '#6da7ec', '#9ec5f4'] as const;

// ロゴの地色。ブランドの色なので他には使わない。
export const brand = { claude: '#d97757', codex: '#d0d0d0' } as const;

// ポップオーバーとモーダルだけに使う幕と影。
export const overlay = { scrim: '#00000073', shadow: '#0008' } as const;

// イベントの色。判別できる色相は 5 つまで（全ペアで ΔE 17.2、検証器の下限 15）。
// 応答・思考・その他は無彩色の濃淡。Claude の橙・Codex の青緑・失敗の赤は使わない。
export const eventGroups = {
  user: { label: 'ユーザー', color: '#aa8d00' },
  assistant: { label: '応答', color: '#b3b7c1' },
  thinking: { label: '思考', color: '#565a62' },
  reference: { label: '参照', color: '#00719e' },
  edit: { label: '編集', color: '#477200' },
  shell: { label: 'シェル', color: '#967cdb' },
  agent: { label: 'エージェント', color: '#953d7c' },
  other: { label: 'その他', color: '#82868f' },
} as const;

export type EventGroup = keyof typeof eventGroups;

const kind = <G extends EventGroup, I extends string>(label: string, group: G, icon: I) => ({
  label,
  group,
  color: eventGroups[group].color,
  icon,
});

export const eventKinds = {
  user: kind('ユーザー', 'user', 'User'),
  assistant: kind('応答', 'assistant', 'MessageSquare'),
  thinking: kind('思考', 'thinking', 'Brain'),
  read: kind('読み取り', 'reference', 'FileText'),
  edit: kind('編集', 'edit', 'FilePen'),
  shell: kind('シェル', 'shell', 'Terminal'),
  web: kind('Web', 'reference', 'Globe'),
  plan: kind('計画', 'agent', 'ListChecks'),
  agent: kind('サブエージェント', 'agent', 'Bot'),
  other: kind('その他', 'other', 'Wrench'),
} as const;

export type EventKind = keyof typeof eventKinds;

// ツール名ごとのアイコン。種類（eventKinds）の既定より細かく分ける。
// 名前が無いもの（mcp__ で始まるものは Plug）は種類の既定を使う。src/theme.rs の tool_icon と一致を検査する。
export const toolIcons = {
  Read: 'FileText',
  read_file: 'FileText',
  Grep: 'TextSearch',
  grep_files: 'TextSearch',
  Glob: 'FolderSearch',
  LS: 'FolderSearch',
  list_dir: 'FolderSearch',
  view_image: 'Image',
  Edit: 'FilePen',
  MultiEdit: 'FilePen',
  apply_patch: 'FilePen',
  Write: 'FilePlus',
  NotebookEdit: 'NotebookPen',
  Bash: 'Terminal',
  BashOutput: 'Terminal',
  exec: 'Terminal',
  exec_command: 'Terminal',
  shell: 'Terminal',
  local_shell_call: 'Terminal',
  write_stdin: 'Keyboard',
  wait: 'Hourglass',
  Monitor: 'Activity',
  WebSearch: 'Globe',
  web_search_call: 'Globe',
  web_search: 'Globe',
  WebFetch: 'CloudDownload',
  TodoWrite: 'ListChecks',
  update_plan: 'ListChecks',
  TaskCreate: 'ListChecks',
  TaskUpdate: 'ListChecks',
  TaskList: 'ListChecks',
  ExitPlanMode: 'Map',
  update_goal: 'Target',
  Skill: 'Sparkles',
} as const;

// プロジェクトの目印の色。名前を必ず並べるので、同じ色になっても構わない。
export const projectColors = [
  '#5d7fb0',
  '#5e9a82',
  '#b08f55',
  '#a66f97',
  '#b8714f',
  '#4f97a6',
  '#949a52',
  '#b35f70',
] as const;
