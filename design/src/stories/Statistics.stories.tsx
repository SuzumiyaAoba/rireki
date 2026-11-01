import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, within } from 'storybook/test';
import { RirekiApp } from '../RirekiApp';

const meta = {
  title: '画面/統計',
  component: RirekiApp,
  render: (args) => <RirekiApp key={JSON.stringify(args)} {...args} />,
  args: { initialView: 'statistics' },
  parameters: {
    docs: {
      description: {
        component:
          'src/app/statistics.rs と statistics/ 以下に対応。値は共通の固定サンプルから算出します。',
      },
    },
  },
} satisfies Meta<typeof RirekiApp>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Overview: Story = { name: '全体' };
export const Claude: Story = { name: 'Claude Code', args: { initialSource: 'claude' } };
export const Project: Story = { name: 'プロジェクトの統計', args: { initialProject: 'rireki' } };
export const DrillDown: Story = {
  name: '操作：指標を選びセッションを開く',
  play: async ({ canvas, userEvent }) => {
    await userEvent.click(canvas.getByRole('button', { name: /失敗・拒否・中断/ }));
    await expect(canvas.getByText('失敗・拒否・中断の推移')).toBeVisible();
    await userEvent.click(canvas.getByRole('button', { name: /タイムラインの表示を改善/ }));
    const detail = within(canvas.getByRole('complementary', { name: 'セッション詳細' }));
    await expect(detail.getByText('経過時間')).toBeVisible();
    await expect(canvas.getByRole('button', { name: 'タイムライン' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
  },
};
