import type { EventKind } from './tokens';

export type Source = 'all' | 'claude' | 'codex';
export type View = 'timeline' | 'day' | 'week' | 'month' | 'statistics';
export type DetailTab = 'overview' | 'history' | 'files' | 'analysis';
export type Overlay = 'display' | 'range' | 'help' | 'mcp' | null;

export interface HistoryEvent {
  id: string;
  kind: EventKind;
  name: string;
  summary: string;
  start: number;
  duration: number;
  status: 'success' | 'failed' | 'rejected' | 'interrupted';
  input: string;
  output: string;
}

export interface Session {
  id: string;
  title: string;
  project: string;
  source: Exclude<Source, 'all'>;
  day: number;
  start: number;
  end: number;
  live: boolean;
  tokens: number;
  cost: number;
  agents: string[];
  events: HistoryEvent[];
}

export const projects = [
  { name: 'rireki', color: '#5d7fb0' },
  { name: 'website', color: '#5e9a82' },
  { name: 'dotfiles', color: '#b08f55' },
  { name: 'api-server', color: '#a66f97' },
  { name: 'notes', color: '#8270c9' },
  { name: 'design-system', color: '#b8714f' },
] as const;

const eventTemplates: Array<Pick<HistoryEvent, 'kind' | 'name' | 'summary' | 'input' | 'output'>> =
  [
    {
      kind: 'user',
      name: 'User',
      summary: 'タイムラインの表示を改善してください',
      input:
        'タイムラインの表示を改善してください。サブエージェントの前後関係が分かるようにしたいです。',
      output: '',
    },
    {
      kind: 'thinking',
      name: 'Thinking',
      summary: '既存のレイアウトと描画処理を確認する',
      input: 'レーンの親子関係とイベントの集約方法を確認し、必要な変更を整理します。',
      output: '',
    },
    {
      kind: 'read',
      name: 'Read',
      summary: 'src/app/timeline.rs',
      input: '{\n  "file_path": "src/app/timeline.rs",\n  "offset": 427,\n  "limit": 120\n}',
      output:
        'pub(super) fn timeline(&mut self, window: &Window, cx: &mut Context<Self>)\n    -> AnyElement {\n    let pw = self.plot_width(window);\n    let ph = self.plot_height(window);\n    let layout = self.rows_in(self.t0, self.t0 + self.spp * pw as f64);\n    // セッションとサブエージェントのレーンを描画\n}',
    },
    {
      kind: 'read',
      name: 'Grep',
      summary: 'レーンと選択範囲の参照を検索',
      input: 'rg -n "Row::Lane|Selection" src/app',
      output:
        'src/app/timeline.rs:633: Row::Lane(t) => t,\nsrc/app/selection.rs:42: pub fn navigate(...)\nsrc/app/detail.rs:407: pub fn detail_session(...)',
    },
    {
      kind: 'shell',
      name: 'Bash',
      summary: 'cargo test --lib --no-default-features',
      input: 'cargo test --lib --no-default-features',
      output:
        'running 24 tests\ntest layout::tests::aggregate_calls ... ok\ntest layout::tests::keep_parent_order ... ok\ntest result: ok. 24 passed; 0 failed; 0 ignored',
    },
    {
      kind: 'edit',
      name: 'Edit',
      summary: 'src/app/timeline.rs の接続線を更新',
      input: 'src/app/timeline.rs\n\n親レーンの起動位置から子レーンへの接続線を追加します。',
      output: 'Successfully updated src/app/timeline.rs',
    },
    {
      kind: 'agent',
      name: 'Agent',
      summary: '描画処理のレビュー',
      input: 'イベントの選択とレーンの関連付けを確認してください。',
      output: '選択範囲を維持し、親子レーン間の移動を確認しました。',
    },
    {
      kind: 'plan',
      name: 'TodoWrite',
      summary: '実装と確認の進捗を更新',
      input:
        '[{"content":"既存の描画を確認","status":"completed"},{"content":"接続線を追加","status":"completed"},{"content":"テストを実行","status":"completed"}]',
      output: 'Todos have been modified successfully.',
    },
    {
      kind: 'assistant',
      name: 'Assistant',
      summary: 'サブエージェントの接続線を追加しました',
      input:
        'サブエージェントを起動した位置から子レーンまで接続線を表示しました。選択したイベントの時間範囲もハイライトされます。\n\n`cargo test --lib --no-default-features` はすべて成功しています。',
      output: '',
    },
  ];

function makeEvents(
  id: string,
  start: number,
  end: number,
  source: Session['source'],
  seed: number,
): HistoryEvent[] {
  const count = 16 + (seed % 4) * 7;
  const step = (end - start) / count;
  return Array.from({ length: count }, (_, index) => {
    const template = eventTemplates[index % eventTemplates.length];
    const name =
      source === 'codex'
        ? ({
            Read: 'read_file',
            Grep: 'grep_files',
            Bash: 'exec_command',
            Edit: 'apply_patch',
            Agent: 'spawn_agent',
            TodoWrite: 'update_plan',
          }[template.name] ?? template.name)
        : template.name;
    const status =
      index === 13 && seed % 3 === 0
        ? 'failed'
        : index === 22 && seed % 2 === 0
          ? 'interrupted'
          : 'success';
    return {
      ...template,
      name,
      id: `${id}-event-${index}`,
      start: start + step * index,
      duration: Math.max(0.15 / 3600, step * (index % 4 === 0 ? 0.85 : 0.58)),
      status,
      ...(status === 'failed'
        ? {
            output:
              'error[E0308]: mismatched types\nexpected `Selection`, found `Option<Selection>`\n\nerror: could not compile `rireki` due to 1 previous error',
          }
        : {}),
    };
  });
}

// 固定の架空データ。ローカルのセッションログ、キャッシュ、MCP サーバーには接続しない。
const sessionSeeds: Array<Omit<Session, 'events'>> = [
  {
    id: 'demo-01',
    title: 'タイムラインの表示を改善',
    project: 'rireki',
    source: 'claude',
    day: 0,
    start: 9.25,
    end: 11.6,
    live: false,
    tokens: 482_300,
    cost: 2.84,
    agents: ['描画処理のレビュー', 'テストの確認'],
  },
  {
    id: 'demo-02',
    title: 'カレンダーのレイアウトを調整',
    project: 'rireki',
    source: 'codex',
    day: 0,
    start: 12.4,
    end: 14.35,
    live: false,
    tokens: 326_800,
    cost: 1.27,
    agents: [],
  },
  {
    id: 'demo-03',
    title: '統計画面に失敗率を追加',
    project: 'rireki',
    source: 'codex',
    day: 0,
    start: 14.6,
    end: 17.2,
    live: true,
    tokens: 598_400,
    cost: 2.32,
    agents: ['統計の集計を確認'],
  },
  {
    id: 'demo-04',
    title: 'トップページの余白を調整',
    project: 'website',
    source: 'claude',
    day: 0,
    start: 9.7,
    end: 10.9,
    live: false,
    tokens: 184_200,
    cost: 1.08,
    agents: [],
  },
  {
    id: 'demo-05',
    title: '検索フォームのアクセシビリティ',
    project: 'website',
    source: 'codex',
    day: 0,
    start: 13,
    end: 14.7,
    live: false,
    tokens: 213_600,
    cost: 0.83,
    agents: ['フォームのレビュー'],
  },
  {
    id: 'demo-06',
    title: 'Neovim の設定を整理',
    project: 'dotfiles',
    source: 'claude',
    day: 0,
    start: 11,
    end: 12.15,
    live: false,
    tokens: 92_400,
    cost: 0.54,
    agents: [],
  },
  {
    id: 'demo-07',
    title: '認証ミドルウェアのテストを追加',
    project: 'api-server',
    source: 'codex',
    day: 0,
    start: 10.3,
    end: 12.65,
    live: false,
    tokens: 347_200,
    cost: 1.35,
    agents: [],
  },
  {
    id: 'demo-08',
    title: 'API ドキュメントを更新',
    project: 'api-server',
    source: 'claude',
    day: 0,
    start: 14,
    end: 16.4,
    live: false,
    tokens: 271_900,
    cost: 1.6,
    agents: ['仕様の確認'],
  },
  {
    id: 'demo-09',
    title: '開発メモを整理',
    project: 'notes',
    source: 'claude',
    day: 0,
    start: 16,
    end: 16.65,
    live: false,
    tokens: 48_600,
    cost: 0.29,
    agents: [],
  },
  {
    id: 'demo-10',
    title: 'ボタンの状態と配色を整理',
    project: 'design-system',
    source: 'codex',
    day: 0,
    start: 15.1,
    end: 17.15,
    live: true,
    tokens: 238_700,
    cost: 0.93,
    agents: [],
  },
  {
    id: 'demo-11',
    title: 'ファイル変更履歴の差分表示',
    project: 'rireki',
    source: 'claude',
    day: 1,
    start: 9.5,
    end: 12.25,
    live: false,
    tokens: 318_200,
    cost: 1.88,
    agents: ['差分のレビュー'],
  },
  {
    id: 'demo-12',
    title: 'ツールの所要時間を比較',
    project: 'rireki',
    source: 'codex',
    day: 2,
    start: 13.25,
    end: 16.25,
    live: false,
    tokens: 286_400,
    cost: 1.11,
    agents: [],
  },
];

export const sessions: Session[] = sessionSeeds.map((session, index) => ({
  ...session,
  events: makeEvents(session.id, session.start, session.end, session.source, index),
}));

export function filterSessions(source: Source, project: string | null, query = '') {
  const terms = query.toLocaleLowerCase().trim().split(/\s+/).filter(Boolean);
  return sessions.filter(
    (session) =>
      (source === 'all' || session.source === source) &&
      (!project || session.project === project) &&
      terms.every((term) =>
        `${session.title} ${session.project} ${session.id} ${session.events.map((e) => `${e.name} ${e.summary}`).join(' ')}`
          .toLocaleLowerCase()
          .includes(term),
      ),
  );
}

export function projectColor(name: string) {
  return projects.find((project) => project.name === name)?.color ?? '#737d8f';
}

export const formatNumber = (value: number) => new Intl.NumberFormat('ja-JP').format(value);
export const formatTokens = (value: number) =>
  value >= 1_000_000 ? `${(value / 1_000_000).toFixed(2)}M` : `${(value / 1_000).toFixed(1)}K`;
export function formatTime(hour: number) {
  const minutes = ((Math.round(hour * 60) % 1440) + 1440) % 1440;
  return `${String(Math.floor(minutes / 60)).padStart(2, '0')}:${String(minutes % 60).padStart(2, '0')}`;
}
export function formatDuration(hours: number) {
  const minutes = Math.round(hours * 60);
  return minutes >= 60 ? `${Math.floor(minutes / 60)}時間 ${minutes % 60}分` : `${minutes}分`;
}
export function formatDate(day: number, compact = false) {
  const date = new Date(Date.UTC(2026, 9, 26 + day));
  return compact
    ? `${date.getUTCMonth() + 1}/${date.getUTCDate()}`
    : `${date.getUTCFullYear()}/${date.getUTCMonth() + 1}/${date.getUTCDate()}`;
}

export const changedFiles = [
  { path: 'src/app/timeline.rs', added: 38, removed: 12 },
  { path: 'src/app/selection.rs', added: 14, removed: 6 },
  { path: 'src/theme.rs', added: 3, removed: 1 },
];

export const diffLines = [
  { kind: 'context', text: '    let selected = self.selected;' },
  { kind: 'context', text: '    let mut row_y_of_lane = HashMap::new();' },
  { kind: 'removed', text: '    for row in tracks.iter() {' },
  { kind: 'added', text: '    for (row, track) in tracks.iter().enumerate() {' },
  { kind: 'added', text: '        let y = row as f32 * ROW_H;' },
  { kind: 'added', text: '        row_y_of_lane.insert(track.key(), y);' },
  { kind: 'context', text: '        self.paint_lane(track, selected);' },
  { kind: 'context', text: '    }' },
  { kind: 'added', text: '' },
  { kind: 'added', text: '    // 起動位置から子レーンへの接続線' },
  { kind: 'added', text: '    self.paint_agent_links(&row_y_of_lane);' },
] as const;
