import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import type { Overlay, Session } from '../data';
import type { Granularity, TimeRange } from './Timeline';
import { StatusText, Button, CopyButton, Icon, IconButton, Segmented } from './primitives';

function Modal({
  title,
  children,
  onClose,
  className = '',
}: {
  title: string;
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
          <h2 id={titleId}>{title}</h2>
          <IconButton icon="X" label={`${title}を閉じる`} onClick={onClose} />
        </header>
        {children}
      </div>
    </div>
  );
}

export const mcpTools: Array<{ name: string; description: string }> = [
  { name: 'list_sessions', description: 'セッションを検索・一覧取得' },
  { name: 'get_session', description: '概要・サブエージェント・使用量を取得' },
  { name: 'get_events', description: '対話本文・ツールの入力と結果を取得' },
  { name: 'search_events', description: 'イベントの名前・要約・モデルを検索' },
  {
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
    <Modal title="MCP サーバー" onClose={onClose} className="mcp-modal">
      <div className="modal-content">
        <div className="mcp-status">
          <span className={`status-dot ${running ? 'running' : ''}`} />
          <strong>{running ? '実行中' : '停止中'}</strong>
          <StatusText>Streamable HTTP</StatusText>
        </div>
        <div className="mcp-port">
          <label htmlFor="mcp-port">ポート番号</label>
          <input
            id="mcp-port"
            inputMode="numeric"
            value={port}
            disabled={running}
            aria-invalid={!valid}
            onChange={(event) => setPort(event.target.value)}
          />
        </div>
        {!valid && (
          <p className="field-error" role="alert">
            1〜65535 のポート番号を入力してください
          </p>
        )}
        <div className="endpoint">
          <span>エンドポイント</span>
          <code title="この端末からの接続だけを受け付け、履歴の読み取り以外は行いません。">
            {endpoint}
          </code>
          <CopyButton text={endpoint} label="URL をコピー" />
        </div>
        <div className="mcp-config">
          <div>
            <strong>クライアント設定</strong>
            <CopyButton text={config} label="接続設定をコピー" />
          </div>
          <pre>
            <code>{config}</code>
          </pre>
        </div>
        <h3 className="detail-label">ツール</h3>
        <div className="mcp-tools">
          {mcpTools.map((tool) => (
            <div key={tool.name} title={tool.description}>
              <code>{tool.name}</code>
            </div>
          ))}
        </div>
      </div>
      <footer className="modal-footer">
        <Button
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
      ['日', '⌘2 · D'],
      ['週', '⌘3 · W'],
      ['月', 'M'],
      ['統計', '⌘4'],
      ['サイドバーの表示を切り替え', '⌃⌘S · ⌘B'],
      ['表示範囲内のセッションだけを表示', 'V'],
    ],
  },
  {
    title: '移動',
    rows: [
      ['今日（現在時刻）へ', '⌘T · T'],
      ['前 / 次の期間', '⌘← ⌘→ · ← →'],
      ['期間・日付を選ぶ', 'G'],
      ['拡大 / 縮小', '⌘= ⌘- · + −'],
      ['表示範囲を動かす', 'ドラッグ · スクロール'],
      ['マウス位置を中心に拡大縮小', '⌘ + スクロール · ピンチ'],
      ['ブロックの範囲まで拡大（カレンダーではタイムラインで開く）', 'ダブルクリック'],
    ],
  },
  {
    title: '選択',
    rows: [
      ['詳細を開く', 'クリック'],
      ['次 / 前のイベント', 'J · K'],
      ['詳細のタブ（セッション / イベント / ファイル / 分析）', '1 · 2 · 3 · 4'],
      ['詳細の全画面表示 / 通常表示', '⇧⌘↵'],
      ['選択範囲のイベントを検索', '⌘F（分析タブ）'],
      ['選択の履歴を戻る / 進む', '⌘[ ⌘]'],
      ['全画面を解除 / 詳細を閉じる', 'esc'],
    ],
  },
  {
    title: '検索',
    rows: [
      ['検索欄へ', '⌘K · ⌘F'],
      ['結果を選ぶ', '↑ ↓'],
      ['結果を開く', 'enter'],
      ['検索を解除', 'esc'],
    ],
  },
];

export function HelpDialog({ onClose }: { onClose: () => void }) {
  return (
    <Modal title="キーボードショートカット" onClose={onClose} className="help-modal">
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
        <div className="display-option-row">
          <span>表示内容</span>
          <Segmented
            label="表示内容"
            toolbar
            options={[
              { value: 'events', label: 'イベント' },
              { value: 'cost', label: '料金' },
            ]}
            value={cost ? 'cost' : 'events'}
            onChange={(value) => onCost(value === 'cost')}
          />
        </div>
        <div className="display-option-row">
          <span>行の並び</span>
          <Segmented
            label="行の並び"
            toolbar
            options={[
              { value: 'session', label: 'セッション' },
              { value: 'directory', label: 'ディレクトリ' },
            ]}
            value={directory ? 'directory' : 'session'}
            onChange={(value) => onDirectory(value === 'directory')}
          />
        </div>
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
            title={cost ? '料金の表示ではまとめ方を選べません' : description}
            disabled={cost}
            onClick={() => onGranularity(value)}
          >
            <span>{granularity === value && <Icon name="Check" size={13} />}</span>
            <strong>{label}</strong>
          </button>
        ))}
        <hr />
        <label className="checkbox-row">
          <input
            type="checkbox"
            checked={inViewOnly}
            onChange={(event) => onInViewOnly(event.target.checked)}
          />
          <span>表示範囲内のセッションだけ</span>
        </label>
      </div>
    </>
  );
}

// 期間のプリセット。本体 header.rs の PRESETS と同じ。サンプルデータは 3 日分（72 時間）しかないので、
// 日単位の期間と全期間はどれも 72 時間に丸める。
const rangePresets: ReadonlyArray<readonly [string, number]> = [
  ['直近 1 時間', 1],
  ['直近 6 時間', 6],
  ['直近 24 時間', 24],
  ['直近 7 日間', 72],
  ['直近 30 日間', 72],
  ['直近 90 日間', 72],
  ['全期間', 72],
];

export function RangePicker({
  sessions,
  onRange,
  onDay,
  onClose,
}: {
  sessions: Session[];
  onRange: (range: TimeRange) => void;
  onDay: (day: number) => void;
  onClose: () => void;
}) {
  const [month, setMonth] = useState(0);
  const first = new Date(Date.UTC(2026, 9 + month, 1));
  const mondayOffset = (first.getUTCDay() + 6) % 7;
  // サンプルデータの day 0 は 2026-10-26
  const firstDay =
    Math.round((first.getTime() - Date.UTC(2026, 9, 26)) / 86_400_000) - mondayOffset;
  const counts = Array.from({ length: 42 }, (_, i) =>
    sessions
      .filter((session) => session.day === firstDay + i)
      .reduce((sum, session) => sum + session.events.length, 0),
  );
  const most = Math.max(1, ...counts);
  return (
    <>
      <button
        type="button"
        className="popover-dismiss"
        aria-label="期間の選択を閉じる"
        onClick={onClose}
      />
      <div className="range-popover popover" role="dialog" aria-label="期間・日付の選択">
        <div className="range-presets-column">
          <h3>表示する期間</h3>
          <div className="range-presets">
            {rangePresets.map(([label, hours]) => (
              <button
                type="button"
                key={label}
                onClick={() => {
                  onRange({ start: Math.max(0, 18 - hours), end: hours >= 24 ? hours : 18 });
                  onClose();
                }}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
        <div className="range-calendar-column">
          <div className="range-month">
            <IconButton icon="ChevronLeft" label="前の月" onClick={() => setMonth(month - 1)} />
            <strong>
              {first.getUTCFullYear()}年{first.getUTCMonth() + 1}月
            </strong>
            <IconButton icon="ChevronRight" label="次の月" onClick={() => setMonth(month + 1)} />
          </div>
          <div className="range-calendar">
            {['月', '火', '水', '木', '金', '土', '日'].map((day) => (
              <span key={day}>{day}</span>
            ))}
            {counts.map((count, i) => {
              const day = firstDay + i;
              const date = new Date(Date.UTC(2026, 9, 26 + day));
              const inMonth = date.getUTCMonth() === first.getUTCMonth();
              return (
                <button
                  type="button"
                  className={`${inMonth ? '' : 'outside'} ${day === 0 ? 'today' : ''}`}
                  aria-label={`${date.getUTCMonth() + 1}月${date.getUTCDate()}日を選択`}
                  key={day}
                  onClick={() => {
                    onDay(day);
                    onClose();
                  }}
                >
                  {date.getUTCDate()}
                  {/* 日ごとのイベント量。幅は件数の平方根に比例する */}
                  {count > 0 && <i style={{ width: 4 + 18 * Math.sqrt(count / most) }} />}
                </button>
              );
            })}
          </div>
          <p className="range-hint">日付を選ぶと、その日へ移動</p>
        </div>
      </div>
    </>
  );
}

export function isModal(overlay: Overlay) {
  return overlay === 'help' || overlay === 'mcp';
}
