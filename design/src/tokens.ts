// src/theme.rs と src/app.rs の値。GPUI 側を変更した際はここも更新する。
export const colors = {
  backdrop: '#0e1014',
  sidebar: '#15171c',
  bg: '#16181d',
  panel: '#1d2027',
  elevated: '#252933',
  border: '#2c313b',
  grid: '#252932',
  text: '#d7dae0',
  muted: '#8b93a1',
  subtle: '#646c7a',
  accent: '#5b9cf5',
  accentBg: '#24324a',
  select: '#2a5aa8',
  hover: '#262a33',
  live: '#3fb950',
  now: '#e5534b',
  sub: '#b9a3f0',
  ok: '#7fd18b',
  err: '#f07c7c',
  warn: '#e0a24a',
  claude: '#d95926',
  codex: '#199e70',
} as const;

export const dimensions = {
  strip: 34,
  rail: 68,
  gap: 6,
  panelRadius: 10,
  header: 46,
  footer: 26,
  minimap: 30,
  axis: 40,
  row: 32,
  sidebar: 260,
  label: 260,
  detail: 440,
} as const;

export const eventKinds = {
  user: { label: 'User', color: '#e0b341', icon: 'User' },
  assistant: { label: 'Assistant', color: '#4fb3a5', icon: 'MessageSquare' },
  thinking: { label: 'Thinking', color: '#6b6f7a', icon: 'Brain' },
  read: { label: '読み取り', color: '#4a8fe7', icon: 'FileText' },
  edit: { label: '編集', color: '#55b45f', icon: 'FilePen' },
  shell: { label: 'シェル', color: '#e08a3c', icon: 'Terminal' },
  web: { label: 'Web', color: '#3cc3e0', icon: 'Globe' },
  plan: { label: '計画', color: '#d96ba0', icon: 'ListChecks' },
  agent: { label: 'サブエージェント', color: '#a77bf3', icon: 'Bot' },
  other: { label: 'その他', color: '#7f8ea3', icon: 'Wrench' },
} as const;

export type EventKind = keyof typeof eventKinds;
