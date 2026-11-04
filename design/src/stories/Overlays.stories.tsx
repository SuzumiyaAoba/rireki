import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, within } from 'storybook/test';
import { RirekiApp } from '../RirekiApp';

const meta = {
  title: 'パネル/オーバーレイ',
  component: RirekiApp,
  render: (args) => <RirekiApp key={JSON.stringify(args)} {...args} />,
  parameters: {
    docs: {
      description: {
        component:
          'src/app/overlay.rs・range.rs・mcp.rs のポップオーバーとダイアログ。MCP の開始・停止は表示状態のみのシミュレーションで、サーバーは起動しません。',
      },
    },
  },
} satisfies Meta<typeof RirekiApp>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Display: Story = { name: '表示設定', args: { initialOverlay: 'display' } };
export const Range: Story = { name: '期間・日付の選択', args: { initialOverlay: 'range' } };
export const Help: Story = { name: 'キー操作', args: { initialOverlay: 'help' } };
export const McpStopped: Story = { name: 'MCP：停止中', args: { initialOverlay: 'mcp' } };
export const McpRunning: Story = {
  name: 'MCP：実行中',
  args: { initialOverlay: 'mcp', initialMcpRunning: true },
};

export const ChangeDisplay: Story = {
  name: '操作：料金と行の表示',
  args: { initialOverlay: 'display' },
  play: async ({ canvas, userEvent }) => {
    const dialog = within(canvas.getByRole('dialog', { name: '表示設定' }));
    await userEvent.click(dialog.getByRole('button', { name: '料金' }));
    await userEvent.click(dialog.getByRole('button', { name: 'ディレクトリ' }));
    await expect(dialog.getByRole('button', { name: /自動/ })).toBeDisabled();
    await userEvent.click(canvas.getByRole('button', { name: '表示設定を閉じる' }));
    await expect(canvas.getByLabelText('料金のタイムライン')).toBeVisible();
    await expect(canvas.getByText('ディレクトリ', { exact: true })).toBeVisible();
  },
};

export const McpInteraction: Story = {
  name: '操作：MCP の状態とポートの入力',
  args: { initialOverlay: 'mcp' },
  play: async ({ canvas, userEvent }) => {
    const dialog = within(canvas.getByRole('dialog', { name: 'MCP サーバー' }));
    await userEvent.clear(dialog.getByRole('textbox', { name: 'ポート番号' }));
    await userEvent.type(dialog.getByRole('textbox', { name: 'ポート番号' }), '70000');
    await expect(dialog.getByRole('button', { name: 'サーバーを起動' })).toBeDisabled();
    await userEvent.clear(dialog.getByRole('textbox', { name: 'ポート番号' }));
    await userEvent.type(dialog.getByRole('textbox', { name: 'ポート番号' }), '47822');
    await userEvent.click(dialog.getByRole('button', { name: 'サーバーを起動' }));
    await expect(dialog.getByText('実行中', { exact: true })).toBeVisible();
    await expect(dialog.getByRole('textbox', { name: 'ポート番号' })).toBeDisabled();
    await userEvent.click(dialog.getByRole('button', { name: 'サーバーを停止' }));
    await expect(dialog.getByText('停止中', { exact: true })).toBeVisible();
    await userEvent.keyboard('{Escape}');
    await expect(canvas.queryByRole('dialog')).not.toBeInTheDocument();
  },
};
