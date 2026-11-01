import { colors, projectColors, type EventKind } from './tokens';

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
  agents: Array<{ name: string; events: number }>;
  events: HistoryEvent[];
  /** キャッシュから読んだ入力の割合（0〜1） */
  cacheHit: number;
  /** トークンの内訳（%、合計 100） */
  usage: { input: number; cache: number; output: number };
}

export const projects = [
  { name: 'rireki', color: projectColors[0] },
  { name: 'kakeibo-web', color: projectColors[1] },
  { name: 'dotfiles', color: projectColors[2] },
  { name: 'ledger-api', color: projectColors[3] },
  { name: 'zettel', color: projectColors[5] },
  { name: 'kumo-ui', color: projectColors[4] },
] as const;

type Template = Pick<HistoryEvent, 'kind' | 'name' | 'summary' | 'input' | 'output'>;
type DiffLine = { kind: 'context' | 'added' | 'removed'; text: string };

/** プロジェクトごとの素材。ツール呼び出しの対象・コマンド・差分をここから選ぶ。 */
interface Kit {
  files: string[];
  snippets: string[];
  searches: Array<{ query: string; hits: string }>;
  commands: Array<{ cmd: string; out: string }>;
  failure: { cmd: string; out: string };
  changes: Array<{ path: string; added: number; removed: number }>;
  diff: DiffLine[];
}

const kits: Record<string, Kit> = {
  rireki: {
    files: [
      'src/app/calendar/day.rs',
      'src/app/statistics.rs',
      'src/detail/summary.rs',
      'src/index/writer.rs',
      'src/search.rs',
    ],
    snippets: [
      'pub fn plan_columns(segments: &[Segment]) -> Vec<Column> {\n    let mut columns: Vec<Column> = Vec::new();\n    // 開始の早い順に、空いている最初の列へ置く\n}',
      'impl Cache {\n    pub fn summarize(&mut self, session: &Arc<Session>) -> Totals {\n        self.entries.entry(session.version()).or_default().clone()\n    }\n}',
      'pub fn session_summary(session: &Session) -> Summary {\n    let mut summary = Summary::default();\n    for lane in &session.lanes { summary.add(lane); }\n    summary\n}',
    ],
    searches: [
      {
        query: 'plan_columns',
        hits: 'src/app/calendar/columns.rs:41: pub fn plan_columns(\nsrc/app/calendar/day.rs:118: let columns = plan_columns(&segments);',
      },
      {
        query: 'Cache::new',
        hits: 'src/statistics/cache.rs:88: pub fn new() -> Self {\nsrc/app/statistics.rs:112: cache: Cache::new(),',
      },
      {
        query: 'LIVE_SECS',
        hits: 'src/loader.rs:57: pub const LIVE_SECS: u64 = 30;\nsrc/index.rs:211: if age < LIVE_SECS {',
      },
    ],
    commands: [
      {
        cmd: 'cargo test --lib --no-default-features',
        out: 'running 31 tests\ntest app::calendar::tests::overlapping_columns ... ok\ntest statistics::tests::failure_rate ... ok\ntest result: ok. 31 passed; 0 failed; 0 ignored',
      },
      {
        cmd: 'cargo clippy',
        out: '    Checking rireki v0.9.0\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.12s',
      },
      {
        cmd: 'cargo build',
        out: '   Compiling rireki v0.9.0\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.48s',
      },
    ],
    failure: {
      cmd: 'cargo test --lib --no-default-features',
      out: 'error[E0382]: borrow of moved value: `session`\n  --> src/detail/summary.rs:73:24\n\nerror: could not compile `rireki` due to 1 previous error',
    },
    changes: [
      { path: 'src/app/calendar/day.rs', added: 24, removed: 7 },
      { path: 'src/app/calendar/columns.rs', added: 11, removed: 3 },
      { path: 'src/app/calendar/tests.rs', added: 18, removed: 0 },
    ],
    diff: [
      { kind: 'context', text: '    let segments = self.segments_in(day);' },
      {
        kind: 'removed',
        text: '    let columns = segments.iter().map(|s| s.start).collect::<Vec<_>>();',
      },
      { kind: 'added', text: '    let columns = plan_columns(&segments);' },
      { kind: 'context', text: '    for (segment, column) in segments.iter().zip(&columns) {' },
      { kind: 'added', text: '        let width = column.span(columns.len());' },
      { kind: 'context', text: '        self.paint_segment(segment, width);' },
      { kind: 'context', text: '    }' },
    ],
  },
  'kakeibo-web': {
    files: [
      'src/pages/Home.tsx',
      'src/styles/home.css',
      'src/components/SearchForm.tsx',
      'src/lib/format.ts',
    ],
    snippets: [
      '.hero {\n  padding: 24px 16px;\n  display: grid;\n  gap: 12px;\n}\n.hero > h1 { font-size: 28px; }',
      'export function SearchForm({ onSearch }: Props) {\n  return <form onSubmit={...}><input name="q" /></form>;\n}',
      'export const yen = (n: number) => `¥${n.toLocaleString("ja-JP")}`;',
    ],
    searches: [
      { query: 'className="hero"', hits: 'src/pages/Home.tsx:18: <section className="hero">' },
      { query: 'aria-label', hits: 'src/components/Header.tsx:9: <nav aria-label="メイン">' },
    ],
    commands: [
      {
        cmd: 'npm run build',
        out: 'vite v5.4.2 building for production...\n✓ 84 modules transformed.\ndist/assets/index-3f9a1c.css   12.4 kB\n✓ built in 1.92s',
      },
      {
        cmd: 'npm test',
        out: ' ✓ src/lib/format.test.ts (4)\n ✓ src/components/SearchForm.test.tsx (3)\n\n Test Files  2 passed (2)',
      },
      { cmd: 'npx tsc --noEmit', out: '' },
    ],
    failure: {
      cmd: 'npx tsc --noEmit',
      out: "src/components/SearchForm.tsx(21,9): error TS2322: Type 'string | null' is not assignable to type 'string'.",
    },
    changes: [
      { path: 'src/styles/home.css', added: 16, removed: 9 },
      { path: 'src/pages/Home.tsx', added: 5, removed: 2 },
    ],
    diff: [
      { kind: 'context', text: '.hero {' },
      { kind: 'removed', text: '  padding: 12px 8px;' },
      { kind: 'added', text: '  padding: 32px 16px;' },
      { kind: 'added', text: '  gap: 12px;' },
      { kind: 'context', text: '}' },
    ],
  },
  dotfiles: {
    files: ['nvim/lua/plugins/lsp.lua', 'nvim/init.lua', 'zsh/.zshrc', 'tmux/tmux.conf'],
    snippets: [
      'return {\n  "neovim/nvim-lspconfig",\n  event = "BufReadPre",\n  config = function() require("lspconfig").lua_ls.setup({}) end,\n}',
      'export EDITOR=nvim\nautoload -Uz compinit && compinit',
    ],
    searches: [
      {
        query: 'lspconfig',
        hits: 'nvim/lua/plugins/lsp.lua:2: "neovim/nvim-lspconfig",\nnvim/init.lua:31: require("lspconfig")',
      },
    ],
    commands: [
      { cmd: 'stylua --check nvim/', out: '' },
      { cmd: 'nvim --headless "+Lazy! sync" +qa', out: 'Updated 3 plugins.' },
      { cmd: 'zsh -n zsh/.zshrc', out: '' },
    ],
    failure: {
      cmd: 'stylua --check nvim/',
      out: 'Diff in nvim/lua/plugins/lsp.lua at line 12:\n-  config = function() require("lspconfig").lua_ls.setup({}) end,\n+  config = function()\n+    require("lspconfig").lua_ls.setup({})\n+  end,',
    },
    changes: [
      { path: 'nvim/lua/plugins/lsp.lua', added: 21, removed: 34 },
      { path: 'nvim/init.lua', added: 2, removed: 18 },
    ],
    diff: [
      { kind: 'removed', text: 'require("lspconfig").lua_ls.setup({})' },
      { kind: 'removed', text: 'require("lspconfig").gopls.setup({})' },
      { kind: 'context', text: 'require("plugins")' },
      { kind: 'added', text: 'require("lazy").setup("plugins")' },
    ],
  },
  'ledger-api': {
    files: [
      'internal/auth/middleware.go',
      'internal/auth/middleware_test.go',
      'docs/openapi.yaml',
      'cmd/server/main.go',
    ],
    snippets: [
      'func RequireToken(next http.Handler) http.Handler {\n\treturn http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {\n\t\ttoken := bearer(r)\n\t\tif _, err := verify(token); err != nil {\n\t\t\thttp.Error(w, "unauthorized", http.StatusUnauthorized)\n\t\t\treturn\n\t\t}\n\t})\n}',
      'paths:\n  /v1/entries:\n    get:\n      summary: 仕訳の一覧',
    ],
    searches: [
      {
        query: 'RequireToken',
        hits: 'internal/auth/middleware.go:14: func RequireToken(\ncmd/server/main.go:48: mux.Handle("/v1/", auth.RequireToken(api))',
      },
      {
        query: 'StatusUnauthorized',
        hits: 'internal/auth/middleware.go:22: http.StatusUnauthorized',
      },
    ],
    commands: [
      {
        cmd: 'go test ./...',
        out: 'ok  \tledger/internal/auth\t0.312s\nok  \tledger/internal/entries\t0.201s',
      },
      { cmd: 'go vet ./...', out: '' },
      { cmd: 'go build ./cmd/server', out: '' },
    ],
    failure: {
      cmd: 'go test ./internal/auth',
      out: '--- FAIL: TestRequireToken_Expired (0.00s)\n    middleware_test.go:58: status = 200, want 401\nFAIL\nFAIL\tledger/internal/auth\t0.287s',
    },
    changes: [
      { path: 'internal/auth/middleware_test.go', added: 64, removed: 0 },
      { path: 'internal/auth/middleware.go', added: 7, removed: 3 },
      { path: 'docs/openapi.yaml', added: 22, removed: 4 },
    ],
    diff: [
      { kind: 'context', text: 'func TestRequireToken_Expired(t *testing.T) {' },
      { kind: 'added', text: '\ttoken := signedToken(t, time.Now().Add(-time.Hour))' },
      { kind: 'added', text: '\trec := serve(t, token)' },
      { kind: 'added', text: '\tif rec.Code != http.StatusUnauthorized {' },
      { kind: 'added', text: '\t\tt.Fatalf("status = %d, want 401", rec.Code)' },
      { kind: 'added', text: '\t}' },
      { kind: 'context', text: '}' },
    ],
  },
  zettel: {
    files: ['inbox/2026-10-24.md', 'projects/ledger-api.md', 'index.md'],
    snippets: [
      '# 2026-10-24\n\n- ledger-api: トークンの期限を 15 分に\n- 読む: SQLite の WAL について',
      '# ledger-api\n\n## 決めたこと\n- 認証は Bearer トークン',
    ],
    searches: [
      {
        query: '^- ledger-api',
        hits: 'inbox/2026-10-24.md:3: - ledger-api: トークンの期限を 15 分に',
      },
    ],
    commands: [
      { cmd: 'git status --short', out: ' M inbox/2026-10-24.md\n M projects/ledger-api.md' },
      { cmd: 'markdownlint "**/*.md"', out: '' },
    ],
    failure: {
      cmd: 'markdownlint "**/*.md"',
      out: 'projects/ledger-api.md:12 MD032/blanks-around-lists Lists should be surrounded by blank lines',
    },
    changes: [
      { path: 'projects/ledger-api.md', added: 9, removed: 2 },
      { path: 'inbox/2026-10-24.md', added: 0, removed: 7 },
    ],
    diff: [
      { kind: 'context', text: '## 決めたこと' },
      { kind: 'added', text: '- トークンの期限は 15 分' },
      { kind: 'context', text: '- 認証は Bearer トークン' },
    ],
  },
  'kumo-ui': {
    files: [
      'src/Button/Button.tsx',
      'src/Button/Button.css',
      'src/Button/Button.stories.tsx',
      'src/tokens/color.ts',
    ],
    snippets: [
      '.button:hover { background: #3b4252; }\n.button:disabled { opacity: 0.5; }',
      "export const color = { surface: '#1f2227', hover: '#32353c', accent: '#5b9cf5' } as const;",
    ],
    searches: [
      {
        query: 'opacity: 0.5',
        hits: 'src/Button/Button.css:14: opacity: 0.5;\nsrc/Input/Input.css:9: opacity: 0.5;',
      },
    ],
    commands: [
      { cmd: 'npm run lint', out: '' },
      {
        cmd: 'npm run test',
        out: ' ✓ src/Button/Button.test.tsx (6)\n\n Test Files  1 passed (1)',
      },
      { cmd: 'npm run build-storybook', out: 'info => Output directory: storybook-static' },
    ],
    failure: {
      cmd: 'npm run lint',
      out: "src/Button/Button.tsx\n  27:9  error  'variant' is assigned a value but never used  @typescript-eslint/no-unused-vars",
    },
    changes: [
      { path: 'src/Button/Button.css', added: 14, removed: 8 },
      { path: 'src/tokens/color.ts', added: 4, removed: 0 },
      { path: 'src/Button/Button.stories.tsx', added: 9, removed: 1 },
    ],
    diff: [
      { kind: 'removed', text: '.button:hover { background: #3b4252; }' },
      { kind: 'added', text: '.button:hover { background: var(--color-hover); }' },
      { kind: 'removed', text: '.button:disabled { opacity: 0.5; }' },
      { kind: 'added', text: '.button:disabled { color: var(--color-muted); }' },
    ],
  },
};

/** デモ 1 だけは、詳細の確認で使う固定の並びを持つ。 */
const demoTemplates: Template[] = [
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

const codexNames: Record<string, string> = {
  Read: 'read_file',
  Grep: 'grep_files',
  Bash: 'exec_command',
  Edit: 'apply_patch',
  Agent: 'spawn_agent',
  TodoWrite: 'update_plan',
};
const toolName = (source: Session['source'], name: string) =>
  source === 'codex' ? (codexNames[name] ?? name) : name;

/** セッションごとの依頼と返答。タイトルと内容が合うように、ここで書く。 */
const talk: Record<string, { prompt: string; thinking: string; reply: string }> = {
  'demo-02': {
    prompt: 'カレンダーの日表示で、重なったセッションが読めません。列を分けてください。',
    thinking: '重なりの判定と列の割り当てを確認する',
    reply: '重なるセッションを別の列に並べ、右の列が空いていれば幅を広げるようにしました。',
  },
  'demo-03': {
    prompt: '統計画面に、ツール呼び出しに対する失敗率を出してください。',
    thinking: '集計キャッシュと指標の定義を確認する',
    reply: '失敗・拒否・中断を指標に加え、呼び出しに対する割合も表示しました。',
  },
  'demo-04': {
    prompt: 'トップページの見出し周りの余白が詰まって見えます。調整してください。',
    thinking: 'ヒーロー部分のスタイルと余白の指定を確認する',
    reply: '見出しと入力欄の間隔を 8px 刻みにそろえ、モバイル幅でも崩れないことを確認しました。',
  },
  'demo-05': {
    prompt: '検索フォームをキーボードだけで操作できるようにしてください。',
    thinking: 'ラベルの関連付けとフォーカス順を確認する',
    reply: 'ラベルを入力欄に関連付け、Enter で検索できるようにしました。',
  },
  'demo-06': {
    prompt: 'LSP の設定が init.lua に散らかっています。プラグインごとにファイルを分けてください。',
    thinking: '設定の読み込み順と依存関係を確認する',
    reply: 'LSP の設定を plugins/lsp.lua に集約し、init.lua を 18 行減らしました。',
  },
  'demo-07': {
    prompt: '認証ミドルウェアに、期限切れトークンと署名違いのテストを足してください。',
    thinking: '既存のテストの書き方とヘルパーを確認する',
    reply:
      '期限切れ・署名違い・ヘッダーなしの 3 ケースを追加し、go test が通ることを確認しました。',
  },
  'demo-08': {
    prompt: '追加したエンドポイントを OpenAPI の定義に反映してください。',
    thinking: 'ルーティングの定義と現在の仕様を突き合わせる',
    reply: '/v1/entries の一覧と作成を openapi.yaml に追加し、レスポンスの例も載せました。',
  },
  'demo-09': {
    prompt: '今週の inbox のメモを、プロジェクトごとのノートに振り分けてください。',
    thinking: 'inbox の項目をプロジェクトに対応づける',
    reply: 'ledger-api に関する 7 件を projects/ledger-api.md に移し、inbox を空にしました。',
  },
  'demo-10': {
    prompt: 'ボタンの hover と disabled の配色を、直書きからトークンに置き換えてください。',
    thinking: '直書きの色の使われ方を洗い出す',
    reply: 'hover と disabled の色をトークンに置き換え、Storybook で見た目を確認しました。',
  },
  'demo-11': {
    prompt: 'ファイルごとの変更履歴で、編集の差分を行番号つきで出してください。',
    thinking: '編集の履歴の持ち方と差分の表示を確認する',
    reply: '編集ごとの差分を行番号つきで展開できるようにしました。',
  },
  'demo-12': {
    prompt: 'ツールごとの所要時間の中央値と P95 を比べられるようにしてください。',
    thinking: '所要時間の集計方法を確認する',
    reply: '中央値と P95 を並べ、共通の対数軸で比べられるようにしました。',
  },
};
const followUps = [
  ['変更点を見て、テストも回してください。', '確認が終わりました。問題は見つかっていません。'],
  ['最後に lint を実行してください。', 'lint を実行しました。警告はありません。'],
  ['テストが通るか確認してください。', 'テストはすべて成功しています。'],
];

// ツールの並び。r 読む / g 検索 / e 編集 / b コマンド。サブエージェントと計画は位置を決めて差し込む
const stepPatterns = ['rgrebeb', 'rrebegb', 'grebrbe', 'rebgrbe'];

function firstSentence(text: string) {
  return text.split('。')[0].replace(/[。.]$/, '');
}

function makeEvents(session: Omit<Session, 'events'>, seed: number): HistoryEvent[] {
  const { id, start, end, source } = session;
  const count = 16 + (seed % 4) * 7;
  const step = (end - start) / count;
  const base = (index: number, template: Template): HistoryEvent => ({
    ...template,
    name: toolName(source, template.name),
    id: `${id}-event-${index}`,
    start: start + step * index,
    duration: Math.max(0.15 / 3600, step * (index % 4 === 0 ? 0.85 : 0.58)),
    status: 'success',
  });
  // デモ 1 は固定の並び（失敗のイベントは 13 番目）。
  if (seed === 0) {
    return Array.from({ length: count }, (_, index) => {
      const template = demoTemplates[index % demoTemplates.length];
      // 2 回目のサブエージェントは、もう一つのレーンの呼び出しにする。
      const event = base(
        index,
        template.kind === 'agent' && index >= demoTemplates.length
          ? {
              ...template,
              summary: session.agents[1].name,
              input: 'テストの実行結果と、失敗したケースを確認してください。',
              output: 'すべてのテストが成功していることを確認しました。',
            }
          : template,
      );
      return index === 13
        ? {
            ...event,
            status: 'failed',
            output:
              'error[E0308]: mismatched types\nexpected `Selection`, found `Option<Selection>`\n\nerror: could not compile `rireki` due to 1 previous error',
          }
        : event;
    });
  }
  const kit = kits[session.project];
  const words = talk[id];
  const pattern = stepPatterns[seed % stepPatterns.length];
  const [followUp, followUpReply] = followUps[seed % followUps.length];
  const twoTurns = count >= 23;
  const secondTurnAt = twoTurns ? Math.floor(count * 0.6) : -1;
  const agentSlots = new Set(session.agents.map((_, i) => Math.floor(count * (0.3 + i * 0.2))));
  // 失敗はコマンドの呼び出しだけに起こすので、サブエージェントの位置は避ける。
  let failureAt = seed % 3 === 0 ? Math.floor(count * 0.5) : -1;
  while (failureAt >= 0 && agentSlots.has(failureAt)) failureAt += 1;
  const agentOf = (index: number) =>
    session.agents[[...agentSlots].sort((a, b) => a - b).indexOf(index)];
  let pick = seed;
  const events: HistoryEvent[] = [];
  for (let index = 0; index < count; index++) {
    pick += 1;
    const file = kit.files[pick % kit.files.length];
    let template: Template;
    if (index === 0) {
      template = {
        kind: 'user',
        name: 'User',
        summary: firstSentence(words.prompt),
        input: words.prompt,
        output: '',
      };
    } else if (index === 1) {
      template = {
        kind: 'thinking',
        name: 'Thinking',
        summary: words.thinking,
        input: `${words.thinking}。必要な変更を整理します。`,
        output: '',
      };
    } else if (index === secondTurnAt) {
      template = {
        kind: 'user',
        name: 'User',
        summary: firstSentence(followUp),
        input: followUp,
        output: '',
      };
    } else if (index === count - 1) {
      template = {
        kind: 'assistant',
        name: 'Assistant',
        summary: firstSentence(twoTurns ? followUpReply : words.reply),
        input: twoTurns ? followUpReply : words.reply,
        output: '',
      };
    } else if (index === secondTurnAt - 1) {
      template = {
        kind: 'assistant',
        name: 'Assistant',
        summary: firstSentence(words.reply),
        input: words.reply,
        output: '',
      };
    } else if (agentSlots.has(index)) {
      const agent = agentOf(index);
      template = {
        kind: 'agent',
        name: 'Agent',
        summary: agent.name,
        input: `「${agent.name}」を依頼します。変更したファイルと関連する呼び出しを確認してください。`,
        output: `「${agent.name}」が完了しました。指摘は見つかりませんでした。`,
      };
    } else if (index === 4 && seed % 2 === 1) {
      template = {
        kind: 'plan',
        name: 'TodoWrite',
        summary: '作業の進捗を更新',
        input: `[{"content":"${file} を確認","status":"completed"},{"content":"変更を反映","status":"in_progress"},{"content":"テストを実行","status":"pending"}]`,
        output: 'Todos have been modified successfully.',
      };
    } else {
      const letter = index === failureAt ? 'b' : pattern[(index + seed) % pattern.length];
      const search = kit.searches[pick % kit.searches.length];
      const command = index === failureAt ? kit.failure : kit.commands[pick % kit.commands.length];
      template =
        letter === 'r'
          ? {
              kind: 'read',
              name: 'Read',
              summary: file,
              input: `{\n  "file_path": "${file}",\n  "limit": 120\n}`,
              output: kit.snippets[pick % kit.snippets.length],
            }
          : letter === 'g'
            ? {
                kind: 'read',
                name: 'Grep',
                summary: `${search.query} を検索`,
                input: `rg -n "${search.query}"`,
                output: search.hits,
              }
            : letter === 'e'
              ? {
                  kind: 'edit',
                  name: 'Edit',
                  summary: `${file} を更新`,
                  input: `${file}\n\n変更内容: ${words.reply}`,
                  output: `Successfully updated ${file}`,
                }
              : {
                  kind: 'shell',
                  name: 'Bash',
                  summary: command.cmd,
                  input: command.cmd,
                  output: command.out,
                };
    }
    const event = base(index, template);
    events.push(
      index === failureAt
        ? { ...event, status: 'failed' }
        : index === 22 && seed % 2 === 0
          ? { ...event, status: 'interrupted' }
          : event,
    );
  }
  return events;
}

// 固定の架空データ。ローカルのセッションログ、キャッシュ、MCP サーバーには接続しない。
const sessionSeeds: Array<
  Omit<Session, 'events' | 'agents' | 'cacheHit' | 'usage'> & { agents: string[] }
> = [
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
    project: 'kakeibo-web',
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
    project: 'kakeibo-web',
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
    project: 'ledger-api',
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
    project: 'ledger-api',
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
    project: 'zettel',
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
    project: 'kumo-ui',
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

export const sessions: Session[] = sessionSeeds.map((seed, index) => {
  const cacheHit = index === 0 ? 0.842 : 0.7 + ((index * 37) % 25) / 100;
  const output = 8 + ((index * 5) % 9);
  const cache = Math.round(cacheHit * 80);
  const session = {
    ...seed,
    agents: seed.agents.map((name, i) => ({ name, events: 6 + ((index + i * 3) % 7) })),
    cacheHit,
    usage: { input: 100 - cache - output, cache, output },
  };
  return { ...session, events: makeEvents(session, index) };
});

/** デモ 1 の変更。詳細の確認用に固定する。 */
const demoChanges = {
  files: [
    { path: 'src/app/timeline.rs', added: 38, removed: 12 },
    { path: 'src/app/selection.rs', added: 14, removed: 6 },
    { path: 'src/theme.rs', added: 3, removed: 1 },
  ],
  diff: [
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
  ] as DiffLine[],
};

/** 変更したファイルと差分。プロジェクトごとの素材から、セッションごとに行数を変えて返す。 */
export function changesFor(session: Session) {
  const kit = kits[session.project];
  const index = sessions.findIndex((item) => item.id === session.id);
  if (index === 0) return demoChanges;
  const files = kit.changes
    .slice(0, Math.max(1, kit.changes.length - (index % 2)))
    .map((file, i) => ({
      ...file,
      added: file.added + ((index * 5 + i * 3) % 9),
      removed: file.removed ? file.removed + ((index + i) % 4) : 0,
    }));
  return { files, diff: kit.diff };
}

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
  return projects.find((project) => project.name === name)?.color ?? colors.subtle;
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
