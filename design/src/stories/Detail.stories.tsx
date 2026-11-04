import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, within } from 'storybook/test';
import { RirekiApp } from '../RirekiApp';

const meta = {
  title: 'パネル/セッション詳細',
  component: RirekiApp,
  render: (args) => <RirekiApp key={JSON.stringify(args)} {...args} />,
  args: { initialSelection: 'demo-01' },
  parameters: {
    docs: {
      description: {
        component:
          'src/app/detail.rs と detail/ 以下に対応するセッション、イベント、ファイル、分析。全画面化、イベントの移動、差分の開閉、分析の絞り込みを操作できます。',
      },
    },
  },
} satisfies Meta<typeof RirekiApp>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Overview: Story = { name: 'セッション' };
export const History: Story = { name: 'イベント', args: { initialTab: 'history' } };
export const Event: Story = {
  name: 'ツールの入力・結果',
  args: { initialTab: 'history', initialEvent: 2 },
};
export const FailedEvent: Story = {
  name: '失敗したツール',
  args: { initialTab: 'history', initialEvent: 13 },
};
export const Files: Story = { name: 'ファイルと差分', args: { initialTab: 'files' } };
export const Analysis: Story = { name: '分析', args: { initialTab: 'analysis' } };
export const Fullscreen: Story = { name: '全画面', args: { initialFullscreen: true } };
export const Live: Story = { name: '進行中のセッション', args: { initialSelection: 'demo-03' } };

export const FilterAnalysis: Story = {
  name: '操作：分析で失敗を絞り込み',
  args: { initialTab: 'analysis' },
  play: async ({ canvas, userEvent }) => {
    const detail = within(canvas.getByRole('complementary', { name: 'セッション詳細' }));
    await userEvent.click(
      within(detail.getByRole('group', { name: 'イベントの状態' })).getByRole('button', {
        name: '失敗・拒否・中断',
      }),
    );
    await expect(detail.getByRole('button', { name: /Bash.*失敗/ })).toBeVisible();
    await userEvent.type(detail.getByRole('textbox', { name: '分析のイベントを検索' }), '該当なし');
    await expect(detail.getByRole('status')).toHaveTextContent('該当するイベントはありません');
    await userEvent.clear(detail.getByRole('textbox', { name: '分析のイベントを検索' }));
    await userEvent.click(detail.getByRole('button', { name: /Bash.*失敗/ }));
    await expect(detail.getByText(/error\[E0308\]/)).toBeVisible();
  },
};

export const KeyboardAndFullscreen: Story = {
  name: '操作：ショートカットと全画面',
  play: async ({ canvas, userEvent }) => {
    await userEvent.click(canvas.getByRole('tab', { name: 'セッション' }));
    await userEvent.keyboard('3');
    await expect(canvas.getByRole('tab', { name: /ファイル/ })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    await userEvent.click(canvas.getByRole('button', { name: '詳細を全画面表示' }));
    await expect(
      canvas.queryByRole('navigation', { name: 'メインナビゲーション' }),
    ).not.toBeInTheDocument();
    await userEvent.keyboard('{Escape}');
    await expect(canvas.getByRole('navigation', { name: 'メインナビゲーション' })).toBeVisible();
    await expect(canvas.getByRole('complementary', { name: 'セッション詳細' })).toBeVisible();
  },
};
