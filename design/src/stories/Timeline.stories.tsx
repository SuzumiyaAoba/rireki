import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, within } from 'storybook/test';
import { RirekiApp } from '../RirekiApp';

const meta = {
  title: '画面/タイムライン',
  component: RirekiApp,
  render: (args) => <RirekiApp key={JSON.stringify(args)} {...args} />,
  parameters: {
    docs: {
      description: {
        component:
          'Rust / GPUI の現在の UI を、固定の架空データで再現。色は src/theme.rs、寸法は src/app.rs・src/prefs.rs に対応します。実際のログや MCP には接続しません。',
      },
    },
  },
  argTypes: {
    initialView: { control: 'select', options: ['timeline', 'day', 'week', 'month', 'statistics'] },
    initialSource: { control: 'select', options: ['all', 'claude', 'codex'] },
    initialTab: { control: 'select', options: ['overview', 'history', 'files', 'analysis'] },
    initialStatus: { control: 'select', options: ['ready', 'loading', 'empty'] },
    initialOverlay: { control: 'select', options: [null, 'display', 'range', 'help', 'mcp'] },
    initialProject: {
      control: 'select',
      options: [null, 'rireki', 'kakeibo-web', 'dotfiles', 'ledger-api', 'zettel', 'kumo-ui'],
    },
    initialSelection: {
      control: 'select',
      options: [null, 'demo-01', 'demo-02', 'demo-03', 'demo-04'],
    },
    initialEvent: { control: { type: 'number', min: 0, max: 15 } },
  },
  args: {
    initialView: 'timeline',
    initialSource: 'all',
    initialSidebar: true,
    initialStatus: 'ready',
  },
} satisfies Meta<typeof RirekiApp>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = { name: 'すべての履歴' };
export const Project: Story = {
  name: 'プロジェクトで絞り込み',
  args: { initialProject: 'rireki' },
};
export const Claude: Story = { name: 'Claude Code', args: { initialSource: 'claude' } };
export const Codex: Story = { name: 'Codex', args: { initialSource: 'codex' } };
export const Cost: Story = { name: '料金', args: { initialCost: true } };
export const Directory: Story = { name: 'ディレクトリ表示', args: { initialDirectory: true } };
export const SidebarClosed: Story = {
  name: 'サイドバーを閉じた状態',
  args: { initialSidebar: false },
};
export const Search: Story = { name: '検索結果', args: { initialQuery: 'apply_patch' } };

export const SessionNavigation: Story = {
  name: '操作：セッションからイベント・差分へ',
  play: async ({ canvas, userEvent }) => {
    const sidebar = within(canvas.getByRole('complementary', { name: '履歴ナビゲーション' }));
    await userEvent.click(sidebar.getByRole('button', { name: /タイムラインの表示を改善/ }));
    const detail = within(canvas.getByRole('complementary', { name: 'セッション詳細' }));
    await expect(detail.getByText('経過時間')).toBeVisible();
    await userEvent.click(detail.getByRole('tab', { name: /イベント/ }));
    await userEvent.click(detail.getAllByRole('button', { name: /Read/ })[0]);
    await expect(detail.getByText('入力', { exact: true })).toBeVisible();
    await expect(detail.getByText(/file_path/)).toBeVisible();
    await userEvent.click(detail.getByRole('tab', { name: /ファイル/ }));
    await expect(detail.getByText('src/app/timeline.rs', { exact: true })).toBeVisible();
    await expect(detail.getByText('3 ファイルを変更')).toBeVisible();
  },
};

export const FilterInteraction: Story = {
  name: '操作：読み込み元とプロジェクト',
  play: async ({ canvas, userEvent }) => {
    await userEvent.click(canvas.getByRole('button', { name: 'Codex の履歴' }));
    const sidebar = within(canvas.getByRole('complementary', { name: '履歴ナビゲーション' }));
    await userEvent.click(sidebar.getByRole('button', { name: /^rireki / }));
    await expect(sidebar.getByText('rireki のセッション')).toBeVisible();
    await expect(
      sidebar.queryByRole('button', { name: /タイムラインの表示を改善/ }),
    ).not.toBeInTheDocument();
    await expect(
      sidebar.getAllByRole('button', { name: /統計画面に失敗率を追加/ })[0],
    ).toBeVisible();
    await expect(canvas.getByRole('button', { name: 'Codex の履歴' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
  },
};

export const SearchInteraction: Story = {
  name: '操作：検索と解除',
  play: async ({ canvas, userEvent }) => {
    await userEvent.type(canvas.getByRole('textbox', { name: '履歴を検索' }), '該当しない検索語');
    await expect(canvas.getByRole('status')).toHaveTextContent('一致する履歴はありません');
    await userEvent.click(canvas.getByRole('button', { name: '検索を解除' }));
    await expect(canvas.getByLabelText('イベントのタイムライン')).toBeVisible();
    await userEvent.type(canvas.getByRole('textbox', { name: '履歴を検索' }), 'apply_patch');
    const sidebar = within(canvas.getByRole('complementary', { name: '履歴ナビゲーション' }));
    await userEvent.click(sidebar.getAllByRole('button', { name: /apply_patch/ })[0]);
    const detail = within(canvas.getByRole('complementary', { name: 'セッション詳細' }));
    await expect(detail.getByRole('tab', { name: /イベント/ })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    await expect(detail.getByText('入力', { exact: true })).toBeVisible();
  },
};
