import type { Meta, StoryObj } from '@storybook/react-vite';
import { colors, dimensions, eventKinds } from '../tokens';

const meta = {
  title: 'デザイントークン/テーマ',
  parameters: {
    docs: {
      description: {
        component:
          '既存の GPUI 実装から転記した色と寸法の一覧。src/theme.rs、src/app.rs、src/prefs.rs を参照してください。',
      },
    },
  },
} satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;
export const Colors: Story = {
  name: '配色',
  render: () => (
    <div className="token-page">
      <h2>rireki の配色</h2>
      <p>src/theme.rs の色をそのまま使用</p>
      <div className="token-grid">
        {Object.entries(colors).map(([name, value]) => (
          <div className="token-swatch" key={name}>
            <div style={{ background: value }} />
            <span>{name}</span>
            <code>{value}</code>
          </div>
        ))}
      </div>
    </div>
  ),
};
export const EventColors: Story = {
  name: 'イベントの色',
  render: () => (
    <div className="token-page">
      <h2>イベントとツールの色</h2>
      <p>src/theme.rs · event_color / tool_color / LEGEND</p>
      <div className="token-grid">
        {Object.entries(eventKinds).map(([kind, value]) => (
          <div className="token-swatch" key={kind}>
            <div style={{ background: value.color }} />
            <span>{value.label}</span>
            <code>{value.color}</code>
          </div>
        ))}
      </div>
    </div>
  ),
};
export const Dimensions: Story = {
  name: '寸法',
  render: () => (
    <div className="token-page">
      <h2>ウィンドウとパネルの寸法</h2>
      <p>src/app.rs の定数と src/prefs.rs の初期値（px）</p>
      <table className="dimension-table">
        <thead>
          <tr>
            <th>名前</th>
            <th>値</th>
          </tr>
        </thead>
        <tbody>
          {Object.entries(dimensions).map(([name, value]) => (
            <tr key={name}>
              <td>{name}</td>
              <td>{value}px</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  ),
};
