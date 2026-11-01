import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import type { Overlay } from '../data';
import type { Granularity, TimeRange } from './Timeline';
import {
  StatusText,
  Button,
  CopyButton,
  Icon,
  IconButton,
  Segmented,
  type IconName,
} from './primitives';

function Modal({
  title,
  icon,
  subtitle,
  children,
  onClose,
  className = '',
}: {
  title: string;
  icon: IconName;
  subtitle?: string;
  children: ReactNode;
  onClose: () => void;
  className?: string;
}) {
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const previouslyFocused = document.activeElement as HTMLElement | null;
    panel.current?.querySelector<HTMLButtonElement>('button')?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        onClose();
      }
      if (event.key !== 'Tab') return;
      const focusable = Array.from(
        panel.current?.querySelectorAll<HTMLElement>(
          'button:not(:disabled), input:not(:disabled), select, a[href]',
        ) ?? [],
      );
      const first = focusable[0];
      const last = focusable.at(-1);
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    };
    panel.current?.addEventListener('keydown', onKey);
    const current = panel.current;
    return () => {
      current?.removeEventListener('keydown', onKey);
      previouslyFocused?.focus();
    };
  }, [onClose]);
  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={panel}
        className={`modal ${className}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
      >
        <header className="modal-heading">
          <span className="modal-icon">
            <Icon name={icon} size={22} />
          </span>
          <div>
            <h2 id={titleId}>{title}</h2>
            {subtitle && <p>{subtitle}</p>}
          </div>
          <IconButton icon="X" label={`${title}を閉じる`} onClick={onClose} />
        </header>
        {children}
      </div>
    </div>
  );
}

export const mcpTools: Array<{ icon: IconName; name: string; description: string }> = [
  { icon: 'ListChecks', name: 'list_sessions', description: 'セッションを検索・一覧取得' },
  { icon: 'FileText', name: 'get_session', description: '概要・サブエージェント・使用量を取得' },
  { icon: 'MessageSquare', name: 'get_events', description: '対話本文・ツールの入力と結果を取得' },
  { icon: 'Search', name: 'search_events', description: 'イベントの名前・要約・モデルを検索' },
  {
    icon: 'ChartColumn',
    name: 'analyze_history',
    description: '期間・プロジェクト別に実行結果と使用量を集計',
  },
];

export function McpDialog({
  running,
  onRunning,
  onClose,
}: {
  running: boolean;
  onRunning: (running: boolean) => void;
  onClose: () => void;
}) {
  const [port, setPort] = useState('47821');
  const valid = /^\d+$/.test(port) && Number(port) >= 1 && Number(port) <= 65535;
  const endpoint = `http://127.0.0.1:${port}/mcp`;
  const config = JSON.stringify(
    { mcpServers: { rireki: { type: 'http', url: endpoint } } },
    null,
    2,
  );
  return (
    <Modal
      title="MCP サーバー"
      icon="Plug"
      subtitle="Claude Code / Codex の履歴をエージェントに接続"
      onClose={onClose}
      className="mcp-modal"
    >
      <div className="modal-content">
        <div className="mcp-status">
          <span className={`status-dot ${running ? 'running' : ''}`} />
          <strong>{running ? '実行中' : '停止中'}</strong>
          <StatusText>Streamable HTTP</StatusText>
        </div>
        <p className="muted">ローカルのセッション履歴を MCP クライアントから検索・参照できます。</p>
        <div className="mcp-port">
          <label htmlFor="mcp-port">ポート</label>
          <input
            id="mcp-port"
            inputMode="numeric"
            value={port}
            disabled={running}
            aria-invalid={!valid}
            onChange={(event) => setPort(event.target.value)}
          />
          <span>127.0.0.1 にのみ接続を受け付けます</span>
        </div>
        {!valid && (
          <p className="removed" role="alert">
            1〜65535 のポート番号を入力してください
          </p>
        )}
        <div className="endpoint">
          <span>エンドポイント</span>
          <code>{endpoint}</code>
          <CopyButton text={endpoint} label="URL をコピー" />
        </div>
        <div className="mcp-config">
          <div>
            <strong>クライアント設定</strong>
            <CopyButton text={config} label="設定をコピー" />
          </div>
          <pre>
            <code>{config}</code>
          </pre>
        </div>
        <h3 className="detail-label">利用できるツール</h3>
        <div className="mcp-tools">
          {mcpTools.map((tool) => (
            <div key={tool.name}>
              <Icon name={tool.icon} />
              <span>
                <code>{tool.name}</code>
                <small>{tool.description}</small>
              </span>
            </div>
          ))}
        </div>
        <div className="mcp-note">
          <Icon name="CircleAlert" size={15} />
          <p>
            サーバーの実行中は、ローカルのプロセスから読み込み済みの履歴を参照できます。ログの変更やコマンドの実行は行いません。
          </p>
        </div>
      </div>
      <footer className="modal-footer">
        <span>
          {running ? 'パネルを閉じてもサーバーは動作し続けます' : 'サーバーは自動では起動しません'}
        </span>
        <Button
          icon={running ? 'Pause' : 'Play'}
          className={running ? 'danger-button' : 'primary-button'}
          disabled={!valid}
          onClick={() => onRunning(!running)}
        >
          {running ? 'サーバーを停止' : 'サーバーを起動'}
        </Button>
      </footer>
    </Modal>
  );
}

const shortcutGroups = [
  {
    title: '表示',
    rows: [
      ['タイムライン', '⌘1'],
      ['日 / 週 / 月', 'D / W / M'],
      ['統計', '⌘4'],
      ['サイドバーの表示を切り替え', '⌘B'],
    ],
  },
  {
    title: '移動',
    rows: [
      ['今日（現在時刻）へ', 'T'],
      ['前 / 次の期間', '← / →'],
      ['期間・日付を選ぶ', 'G'],
      ['拡大 / 縮小', '+ / −'],
    ],
  },
  {
    title: '選択',
    rows: [
      ['詳細を開く', 'クリック'],
      ['次 / 前のイベント', 'J / K'],
      ['セッション / イベント / ファイル / 分析', '1 / 2 / 3 / 4'],
      ['詳細の全画面表示', '⇧⌘↵'],
      ['詳細・ポップオーバーを閉じる', 'esc'],
    ],
  },
  {
    title: '検索',
    rows: [
      ['検索欄へ', '⌘K / ⌘F'],
      ['結果を開く', 'enter'],
      ['検索を解除', 'esc'],
    ],
  },
];

export function HelpDialog({ onClose }: { onClose: () => void }) {
  return (
    <Modal
      title="キーボードショートカット"
      icon="Keyboard"
      onClose={onClose}
      className="help-modal"
    >
      <div className="modal-content shortcut-groups">
        {shortcutGroups.map((group) => (
          <section key={group.title}>
            <h3>{group.title}</h3>
            {group.rows.map(([label, key]) => (
              <div key={label}>
                <span>{label}</span>
                <kbd>{key}</kbd>
              </div>
            ))}
          </section>
        ))}
      </div>
    </Modal>
  );
}

export interface DisplayOptionsProps {
  cost: boolean;
  directory: boolean;
  granularity: Granularity;
  inViewOnly: boolean;
  onCost: (cost: boolean) => void;
  onDirectory: (directory: boolean) => void;
  onGranularity: (granularity: Granularity) => void;
  onInViewOnly: (value: boolean) => void;
  onClose: () => void;
}

export function DisplayOptions({
  cost,
  directory,
  granularity,
  inViewOnly,
  onCost,
  onDirectory,
  onGranularity,
  onInViewOnly,
  onClose,
}: DisplayOptionsProps) {
  return (
    <>
      <button
        type="button"
        className="popover-dismiss"
        aria-label="表示設定を閉じる"
        onClick={onClose}
      />
      <div className="display-popover popover" role="dialog" aria-label="表示設定">
        <h3>表示</h3>
        <div className="display-option-row">
          <span>表示内容</span>
          <Segmented
            label="表示内容"
            options={[
              { value: 'events', label: 'イベント' },
              { value: 'cost', label: '料金' },
            ]}
            value={cost ? 'cost' : 'events'}
            onChange={(value) => onCost(value === 'cost')}
          />
        </div>
        <p>
          {cost ? '区間ごとの料金（USD）を棒で表示' : 'ツール呼び出しやメッセージをブロックで表示'}
        </p>
        <div className="display-option-row">
          <span>行の並び</span>
          <Segmented
            label="行の並び"
            options={[
              { value: 'session', label: 'セッション' },
              { value: 'directory', label: 'ディレクトリ' },
            ]}
            value={directory ? 'directory' : 'session'}
            onChange={(value) => onDirectory(value === 'directory')}
          />
        </div>
        <p>
          {directory
            ? 'メインのイベントだけを詰めて並べる'
            : 'セッションとサブエージェントを 1 行ずつ並べる'}
        </p>
        <hr />
        <h3>ブロックのまとめ方</h3>
        {(
          [
            ['auto', '自動', '拡大率に応じて、近いイベントをまとめる'],
            ['same-tool', '同一ツール', '同じツールの連続した呼び出しをまとめる'],
            ['turn', 'ターン', 'ユーザーの発話から次の発話までをまとめる'],
            ['call', 'コール', 'まとめずに 1 呼び出しずつ表示する'],
          ] as const
        ).map(([value, label, description]) => (
          <button
            type="button"
            className="granularity-option"
            key={value}
            aria-pressed={granularity === value}
            disabled={cost}
            onClick={() => onGranularity(value)}
          >
            <span>{granularity === value && <Icon name="Check" size={13} />}</span>
            <strong>{label}</strong>
            <small>{description}</small>
          </button>
        ))}
        <hr />
        <label className="checkbox-row">
          <input
            type="checkbox"
            checked={inViewOnly}
            onChange={(event) => onInViewOnly(event.target.checked)}
          />
          <span>
            表示範囲内のセッションだけを表示<small>期間外のセッションを一覧から省きます</small>
          </span>
        </label>
      </div>
    </>
  );
}

export function RangePicker({
  range,
  onRange,
  onDay,
  onClose,
}: {
  range: TimeRange;
  onRange: (range: TimeRange) => void;
  onDay: (day: number) => void;
  onClose: () => void;
}) {
  const [start, setStart] = useState(`${String(Math.floor(range.start % 24)).padStart(2, '0')}:00`);
  const [end, setEnd] = useState(`${String(Math.floor(range.end % 24)).padStart(2, '0')}:00`);
  const toHour = (time: string) => {
    const [hour, minute] = time.split(':').map(Number);
    return hour + minute / 60;
  };
  const valid = !!start && !!end && toHour(end) > toHour(start);
  return (
    <>
      <button
        type="button"
        className="popover-dismiss"
        aria-label="期間の選択を閉じる"
        onClick={onClose}
      />
      <div className="range-popover popover" role="dialog" aria-label="期間・日付の選択">
        <h3>表示する期間</h3>
        <div className="range-presets">
          {[
            ['1時間', 1],
            ['3時間', 3],
            ['6時間', 6],
            ['12時間', 12],
            ['24時間', 24],
            ['全期間', 72],
          ].map(([label, hours]) => (
            <Button
              key={label}
              onClick={() => {
                onRange({
                  start: Math.max(0, 18 - Number(hours)),
                  end: Number(hours) >= 24 ? Number(hours) : 18,
                });
                onClose();
              }}
            >
              {label}
            </Button>
          ))}
        </div>
        <hr />
        <div className="range-month">
          <Icon name="Calendar" />
          <strong>2026年10月</strong>
        </div>
        <div className="range-calendar">
          {['月', '火', '水', '木', '金', '土', '日'].map((day) => (
            <span key={day}>{day}</span>
          ))}
          {Array.from({ length: 35 }, (_, i) => {
            const day = i - 2;
            return (
              <button
                type="button"
                className={day === 26 ? 'selected' : day <= 0 || day > 31 ? 'outside' : ''}
                aria-label={`10月${day}日を選択`}
                key={i}
                onClick={() => {
                  onDay(day - 26);
                  onClose();
                }}
              >
                {day <= 0 ? 30 + day : day > 31 ? day - 31 : day}
              </button>
            );
          })}
        </div>
        <hr />
        <div className="range-inputs">
          <label>
            開始
            <input type="time" value={start} onChange={(event) => setStart(event.target.value)} />
          </label>
          <span>—</span>
          <label>
            終了
            <input type="time" value={end} onChange={(event) => setEnd(event.target.value)} />
          </label>
        </div>
        <Button
          className="primary-button range-apply"
          disabled={!valid}
          onClick={() => {
            onRange({ start: toHour(start), end: toHour(end) });
            onClose();
          }}
        >
          期間を適用
        </Button>
      </div>
    </>
  );
}

export function isModal(overlay: Overlay) {
  return overlay === 'help' || overlay === 'mcp';
}
