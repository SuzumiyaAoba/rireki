import type { Meta, StoryObj } from '@storybook/react-vite';
import {
  alpha,
  colors,
  dimensions,
  eventGroups,
  fontSize,
  projectColors,
  radius,
  space,
} from '../tokens';

const meta = {
  title: 'デザイントークン/テーマ',
  parameters: {
    docs: {
      description: {
        component:
          '色・角丸・文字・余白の一覧。src/tokens.ts が正で、README の「デザイン原則」に従って使う。',
      },
    },
  },
} satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;

function Swatches({ items }: { items: Array<[string, string]> }) {
  return (
    <div className="token-grid">
      {items.map(([name, value]) => (
        <div className="token-swatch" key={name}>
          <div style={{ background: value }} />
          <span>{name}</span>
          <code>{value}</code>
        </div>
      ))}
    </div>
  );
}

function ScaleTable({ rows, unit }: { rows: Array<[string, number]>; unit: string }) {
  return (
    <table className="dimension-table">
      <thead>
        <tr>
          <th>名前</th>
          <th>値</th>
        </tr>
      </thead>
      <tbody>
        {rows.map(([name, value]) => (
          <tr key={name}>
            <td>{name}</td>
            <td>
              {value}
              {unit}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export const Colors: Story = {
  name: '配色',
  render: () => (
    <div className="token-page">
      <h2>配色</h2>
      <Swatches items={Object.entries(colors)} />
    </div>
  ),
};
export const EventColors: Story = {
  name: 'イベントの色',
  render: () => (
    <div className="token-page">
      <h2>イベントの色</h2>
      <Swatches items={Object.values(eventGroups).map((group) => [group.label, group.color])} />
      <h2>プロジェクトの色</h2>
      <Swatches items={projectColors.map((color, index) => [`${index + 1}`, color])} />
    </div>
  ),
};
export const Scales: Story = {
  name: '角丸・文字・余白',
  render: () => (
    <div className="token-page">
      <h2>角丸</h2>
      <ScaleTable rows={Object.entries(radius)} unit="px" />
      <h2>文字サイズ</h2>
      <ScaleTable rows={Object.entries(fontSize)} unit="px" />
      <h2>余白</h2>
      <ScaleTable rows={Object.entries(space)} unit="px" />
      <h2>透明度</h2>
      <ScaleTable rows={Object.entries(alpha)} unit="" />
    </div>
  ),
};
export const Dimensions: Story = {
  name: '寸法',
  render: () => (
    <div className="token-page">
      <h2>ウィンドウとパネルの寸法</h2>
      <ScaleTable rows={Object.entries(dimensions)} unit="px" />
    </div>
  ),
};
