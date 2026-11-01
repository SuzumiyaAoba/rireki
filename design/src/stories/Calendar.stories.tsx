import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect } from 'storybook/test';
import { RirekiApp } from '../RirekiApp';

const meta = {
  title: '画面/カレンダー',
  component: RirekiApp,
  render: (args) => <RirekiApp key={JSON.stringify(args)} {...args} />,
  args: { initialView: 'week', initialProject: 'rireki' },
  parameters: {
    docs: {
      description: {
        component:
          'src/app/calendar/day.rs・month.rs に対応する日・週・月の表示。日付、データ、現在時刻は 2026/10/26 のサンプルに固定しています。',
      },
    },
  },
} satisfies Meta<typeof RirekiApp>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Week: Story = { name: '週' };
export const Day: Story = { name: '日', args: { initialView: 'day', initialProject: null } };
export const Month: Story = { name: '月', args: { initialView: 'month', initialProject: null } };
export const Selected: Story = { name: 'セッションの詳細', args: { initialSelection: 'demo-01' } };
export const SwitchViews: Story = {
  name: '操作：週・月・日を切り替え',
  play: async ({ canvas, userEvent }) => {
    await userEvent.click(canvas.getByRole('button', { name: '月' }));
    await expect(canvas.getByLabelText('月カレンダー')).toBeVisible();
    await userEvent.click(canvas.getByRole('button', { name: '10月26日を開く' }));
    await expect(canvas.getByLabelText('日カレンダー')).toBeVisible();
    await expect(canvas.getByRole('button', { name: '日' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
  },
};
