import type { Meta, StoryObj } from '@storybook/react-vite';
import { RirekiApp } from '../RirekiApp';

const meta = {
  title: '画面/状態',
  component: RirekiApp,
  render: (args) => <RirekiApp key={JSON.stringify(args)} {...args} />,
} satisfies Meta<typeof RirekiApp>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Loading: Story = { name: '読み込み中', args: { initialStatus: 'loading' } };
export const Empty: Story = { name: '履歴なし', args: { initialStatus: 'empty' } };
export const NoResults: Story = {
  name: '検索結果なし',
  args: { initialQuery: '該当しない検索語' },
};
export const StatisticsLoading: Story = {
  name: '統計を読み込み中',
  args: { initialView: 'statistics', initialStatus: 'loading' },
};
export const StatisticsEmpty: Story = {
  name: '統計の履歴なし',
  args: { initialView: 'statistics', initialStatus: 'empty' },
};
