import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { eventKinds } from '../tokens';
import {
  StatusText,
  Button,
  EventBlock,
  IconButton,
  ProjectMark,
  Segmented,
  SourceLogo,
  Stat,
} from '../components/primitives';

const meta = {
  title: 'コンポーネント/基本部品',
  tags: ['autodocs'],
  parameters: {
    docs: {
      description: {
        component: 'src/app/widgets.rs、header.rs、theme.rs の共通部品に対応する最小単位の UI。',
      },
    },
  },
} satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;

export const Buttons: Story = {
  name: 'ボタン',
  render: () => (
    <div className="component-preview">
      <Button icon="Terminal">再開コマンドをコピー</Button>
      <Button icon="Play" className="primary-button">
        サーバーを起動
      </Button>
      <Button icon="Pause" className="danger-button">
        サーバーを停止
      </Button>
      <Button icon="FileText" disabled>
        読み込み中
      </Button>
      <IconButton icon="PanelLeftClose" label="サイドバーを隠す" />
      <IconButton icon="Calendar" label="カレンダー" active />
    </div>
  ),
};
export const Events: Story = {
  name: 'イベントブロック',
  render: () => (
    <div className="component-preview column">
      <div className="event-samples">
        {Object.entries(eventKinds).map(([kind, token]) => (
          <div className="event-sample" key={kind}>
            <EventBlock kind={kind as keyof typeof eventKinds} label={token.label} />
          </div>
        ))}
      </div>
      <div className="event-samples">
        <div className="event-sample">
          <EventBlock kind="read" label="Read · 選択中" selected />
        </div>
        <div className="event-sample">
          <EventBlock kind="shell" label="Bash · 失敗" failed />
        </div>
      </div>
    </div>
  ),
};
export const StatusTexts: Story = {
  name: '状態と読み込み元',
  render: () => (
    <div className="component-preview">
      <StatusText tone="live">進行中</StatusText>
      <StatusText>成功</StatusText>
      <StatusText tone="err">失敗</StatusText>
      <StatusText tone="warn">拒否 / 中断</StatusText>
      <SourceLogo source="all" size={36} />
      <SourceLogo source="claude" size={22} />
      <SourceLogo source="codex" size={22} />
      <ProjectMark name="rireki" />
    </div>
  ),
};
export const Stats: Story = {
  name: '詳細の集計',
  render: () => (
    <div className="component-preview">
      <div className="stat-row" style={{ width: 420 }}>
        <Stat label="経過時間" value="2時間 21分" />
        <Stat label="ターン" value="2" />
        <Stat label="失敗等" value="1" color="var(--err)" />
      </div>
    </div>
  ),
};

function SwitchExample() {
  const [mode, setMode] = useState('week');
  return (
    <div className="component-preview">
      <Segmented
        label="カレンダーの表示"
        options={[
          { value: 'day', label: '日' },
          { value: 'week', label: '週' },
          { value: 'month', label: '月' },
        ]}
        value={mode}
        onChange={setMode}
      />
    </div>
  );
}
export const Switch: Story = { name: 'セグメント切り替え', render: () => <SwitchExample /> };
