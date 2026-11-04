import { useEffect, useState, type ReactNode } from 'react';
import {
  changesFor,
  formatDuration,
  formatNumber,
  formatTime,
  formatTokens,
  type DetailTab,
  isFailure,
  type HistoryEvent,
  type Session,
} from '../data';
import { eventGroups, eventKinds } from '../tokens';
import {
  StatusText,
  Button,
  CopyButton,
  EmptyState,
  eventIcon,
  Highlight,
  Icon,
  IconButton,
  ProjectMark,
  Facts,
  Segmented,
  SourceLogo,
} from './primitives';

interface DetailPanelProps {
  session: Session;
  event: HistoryEvent | null;
  tab: DetailTab;
  fullscreen?: boolean;
  onTab: (tab: DetailTab) => void;
  onEvent: (event: HistoryEvent | null) => void;
  onClose: () => void;
  onFullscreen: () => void;
  onLocate?: () => void;
}

export function DetailPanel({
  session,
  event,
  tab,
  fullscreen,
  onTab,
  onEvent,
  onClose,
  onFullscreen,
  onLocate,
}: DetailPanelProps) {
  return (
    <aside className={`detail-panel ${fullscreen ? 'fullscreen' : ''}`} aria-label="セッション詳細">
      <div className="detail-header">
        <IconButton
          icon="ArrowLeft"
          label="セッションの概要に戻る"
          disabled={!event}
          onClick={() => {
            onEvent(null);
            onTab('overview');
          }}
        />
        <span className="detail-spacer" />
        <IconButton icon="LocateFixed" label="選択位置を表示" onClick={onLocate} />
        <IconButton
          icon={fullscreen ? 'Minimize' : 'Maximize'}
          label={fullscreen ? '通常のパネルに戻す' : '詳細を全画面表示'}
          onClick={onFullscreen}
        />
        <IconButton icon="X" label="詳細を閉じる" onClick={onClose} />
      </div>
      <div className="detail-context">
        <div>
          <button type="button" className="context-title" onClick={() => onTab('overview')}>
            {session.title}
          </button>
          {session.live && <StatusText tone="live">進行中</StatusText>}
        </div>
        <div className="context-meta">
          <SourceLogo source={session.source} size={12} />
          <span>{session.source === 'claude' ? 'Claude Code' : 'Codex'}</span>
          <ProjectMark name={session.project} />
          <span>{session.project}</span>
          <small>
            {formatTime(session.start)}–{formatTime(session.end)}
          </small>
        </div>
      </div>
      <div className="detail-tabs" role="tablist" aria-label="詳細のタブ">
        {(
          [
            ['overview', 'セッション', null],
            ['history', 'イベント', session.events.length],
            ['files', 'ファイル', changesFor(session).files.length],
            ['analysis', '分析', null],
          ] as const
        ).map(([value, label, count]) => (
          <button
            type="button"
            role="tab"
            aria-selected={tab === value}
            aria-controls={`detail-${value}`}
            id={`tab-${value}`}
            key={value}
            onClick={() => onTab(value)}
          >
            {label}
            {count !== null && <span>{count}</span>}
          </button>
        ))}
      </div>
      <div
        className="detail-body"
        role="tabpanel"
        id={`detail-${tab}`}
        aria-labelledby={`tab-${tab}`}
      >
        {tab === 'overview' && <Overview session={session} onTab={onTab} onEvent={onEvent} />}
        {tab === 'history' && <History session={session} event={event} onEvent={onEvent} />}
        {tab === 'files' && <Files session={session} />}
        {tab === 'analysis' && (
          <Analysis
            session={session}
            onEvent={(event) => {
              onEvent(event);
              onTab('history');
            }}
          />
        )}
      </div>
    </aside>
  );
}

// GPUI の status_label（src/app/detail.rs）と同じ表示。成功は表示しない（無色）。
// 失敗は err、拒否・中断は warn、結果なしは muted（浮く面にも載るため subtle は使わない）、
// 進行中のセッションの未完了は live の「実行中」。
// 結果の先頭に出る終了コード（Claude の `Exit code 1`、Codex の `[exit 1 · 0.2s]`）。GPUI の exit_code と同じ
const exitCode = (output: string) => /^(?:Exit code |\[exit )(-?\d+)/.exec(output)?.[1];

const statusText = (status: HistoryEvent['status'], live: boolean, output: string) => {
  switch (status) {
    case 'success':
      return null;
    case 'failed': {
      const code = exitCode(output);
      return { label: code ? `失敗 · exit ${code}` : '失敗', tone: 'err' as const };
    }
    case 'rejected':
      return { label: '拒否', tone: 'warn' as const };
    case 'interrupted':
      return { label: '中断', tone: 'warn' as const };
    case 'pending':
      return live
        ? { label: '実行中', tone: 'running' as const }
        : { label: '結果なし', tone: 'muted' as const };
  }
};

function Label({ children }: { children: ReactNode }) {
  return <h3 className="detail-label">{children}</h3>;
}

function Overview({
  session,
  onTab,
  onEvent,
}: Pick<DetailPanelProps, 'session' | 'onTab' | 'onEvent'>) {
  const failures = session.events.filter((event) => isFailure(event.status)).length;
  const turns = session.events.filter((event) => event.kind === 'user');
  // 次の指示より前に応答があれば、その要約を添える。
  const reply = (turn: HistoryEvent) => {
    const next = session.events
      .slice(session.events.indexOf(turn) + 1)
      .find((event) => event.kind === 'assistant' || event.kind === 'user');
    return next?.kind === 'assistant' ? next.summary : undefined;
  };
  return (
    <div className="overview-content">
      <Facts
        items={[
          { label: '開始', value: `2026/10/${26 + session.day} ${formatTime(session.start)}` },
          { label: '経過時間', value: formatDuration(session.end - session.start) },
          { label: 'ターン', value: turns.length },
          {
            label: '失敗等',
            value: failures,
            tone: failures ? 'err' : undefined,
            // GPUI の概要（src/app/detail/overview.rs）と同じ文言。Storybook の指標は押せないので「押すと」の部分は再現しない
            hint:
              failures > 0
                ? '失敗・拒否・中断の合計。押すと最も多いレーンを表示'
                : '失敗・拒否・中断したイベント',
          },
          {
            label: '料金',
            value: `$${session.cost.toFixed(2)}`,
            hint: 'ログの使用量と料金表による API 換算の推定値（USD）',
          },
          { label: 'キャッシュヒット率', value: `${(session.cacheHit * 100).toFixed(1)}%` },
          { label: 'トークン', value: formatNumber(session.tokens) },
          {
            label: 'モデル',
            value: session.source === 'claude' ? 'claude-sonnet-4-5' : 'gpt-5-codex',
          },
          { label: '作業ディレクトリ', value: `~/projects/${session.project}` },
          { label: 'セッション ID', value: <code>{session.id}</code> },
        ]}
      />
      <div className="detail-actions">
        <CopyButton
          label="再開コマンドをコピー"
          text={`${session.source === 'claude' ? 'claude --resume' : 'codex resume'} ${session.id}`}
        />
      </div>
      <Label>指示と応答</Label>
      {turns.map((turn, i) => (
        <button
          type="button"
          className="turn-row"
          key={turn.id}
          onClick={() => {
            onEvent(turn);
            onTab('history');
          }}
        >
          <span className="turn-meta">
            指示 {i + 1} · {formatTime(turn.start)}
          </span>
          <strong>{i === 0 ? session.title : turn.summary}</strong>
          <p>{turn.input}</p>
          {reply(turn) && <p className="turn-response">{reply(turn)}</p>}
        </button>
      ))}
      <Label>レーン</Label>
      <button
        type="button"
        className="lane-row"
        onClick={() => {
          onEvent(null);
          onTab('history');
        }}
      >
        <span>メイン</span>
        <small>{session.events.length} 件</small>
      </button>
      {session.agents.map((agent) => (
        <button
          type="button"
          className="lane-row subagent"
          key={agent.name}
          onClick={() => {
            onEvent(session.events.find((event) => event.kind === 'agent') ?? null);
            onTab('history');
          }}
        >
          <i className="agent-mark" style={{ background: eventGroups.agent.color }} />
          <span>{agent.name}</span>
          <small>{agent.events} 件</small>
        </button>
      ))}
    </div>
  );
}

function CodeBox({ title, text, error = false }: { title: string; text: string; error?: boolean }) {
  return (
    <section className={`code-section ${error ? 'error' : ''}`}>
      <div className="code-heading">
        <Label>{title}</Label>
        <CopyButton text={text} />
      </div>
      <pre>
        <code>{text}</code>
      </pre>
    </section>
  );
}

function History({
  session,
  event,
  onEvent,
}: Pick<DetailPanelProps, 'session' | 'event' | 'onEvent'>) {
  const [failedOnly, setFailedOnly] = useState(false);
  const [reversed, setReversed] = useState(false);
  const [playing, setPlaying] = useState(false);
  const index = event ? session.events.findIndex((item) => item.id === event.id) : -1;
  useEffect(() => {
    if (!playing) return;
    const timer = window.setInterval(() => {
      const next = session.events[index + 1];
      if (next) onEvent(next);
      else setPlaying(false);
    }, 1500);
    return () => window.clearInterval(timer);
  }, [playing, index, session.events, onEvent]);
  const visible = session.events.filter((event) => !failedOnly || isFailure(event.status));
  if (reversed) visible.reverse();
  const outcome = event ? statusText(event.status, session.live, event.output) : null;
  return (
    <div className="history-content">
      <div className="history-toolbar">
        <strong>{event?.name}</strong>
        <span>{event ? `${index + 1} / ${session.events.length}` : `${visible.length} 件`}</span>
        <IconButton
          icon={playing ? 'Pause' : 'Play'}
          label={playing ? '再生を停止' : '履歴を再生'}
          active={playing}
          onClick={() => setPlaying(!playing)}
        />
        <IconButton
          icon="ChevronLeft"
          label="前のイベント"
          disabled={index <= 0}
          onClick={() => onEvent(session.events[index - 1])}
        />
        <IconButton
          icon="ChevronRight"
          label="次のイベント"
          disabled={index >= session.events.length - 1}
          onClick={() => onEvent(session.events[index + 1])}
        />
      </div>
      {event ? (
        <div className="single-event">
          <div className="event-meta">
            10/{26 + session.day} {formatTime(event.start)}
            <span>·</span>
            {Math.round(event.duration * 3600)}秒
            {outcome && <StatusText tone={outcome.tone}>{outcome.label}</StatusText>}
          </div>
          <p className="prompt-reference">
            選択範囲外の指示 <span>{session.title}</span>
          </p>
          <CodeBox
            title={['user', 'assistant', 'thinking'].includes(event.kind) ? event.name : '入力'}
            text={event.input}
          />
          {event.output && (
            <CodeBox title="結果" text={event.output} error={event.status === 'failed'} />
          )}
          <Button
            onClick={() => {
              setPlaying(false);
              onEvent(null);
            }}
          >
            イベント一覧に戻る
          </Button>
        </div>
      ) : (
        <>
          <div className="history-filters">
            <button
              type="button"
              aria-pressed={failedOnly}
              title="失敗・拒否・中断したイベントに絞り込む"
              onClick={() => setFailedOnly(!failedOnly)}
              className={failedOnly ? 'active' : ''}
            >
              失敗等のみ
            </button>
            <button type="button" aria-pressed={reversed} onClick={() => setReversed(!reversed)}>
              {reversed ? '新しい順' : '古い順'}
            </button>
          </div>
          {visible.length ? (
            visible.map((item) => (
              <EventRow
                event={item}
                live={session.live}
                key={item.id}
                onClick={() => onEvent(item)}
              />
            ))
          ) : (
            <EmptyState message="該当するイベントはありません" />
          )}
        </>
      )}
    </div>
  );
}

export function EventRow({
  event,
  query = '',
  live = false,
  onClick,
}: {
  event: HistoryEvent;
  query?: string;
  live?: boolean;
  onClick: () => void;
}) {
  const token = eventKinds[event.kind];
  const outcome = statusText(event.status, live, event.output);
  return (
    <button type="button" className="event-row" onClick={onClick}>
      <span className="event-row-icon" style={{ color: token.color }}>
        <Icon name={eventIcon(event.name, event.kind)} />
      </span>
      <span className="row-content">
        <span className="event-row-title">
          <strong>
            <Highlight text={event.name} query={query} />
          </strong>
          {outcome && <StatusText tone={outcome.tone}>{outcome.label}</StatusText>}
          <small>{formatTime(event.start)}</small>
        </span>
        <span className="truncate">
          <Highlight text={event.summary} query={query} />
        </span>
      </span>
    </button>
  );
}

function Files({ session }: { session: Session }) {
  const { files: changedFiles, diff: diffLines } = changesFor(session);
  const [opened, setOpened] = useState<string[]>([changedFiles[0].path]);
  const [unified, setUnified] = useState(false);
  return (
    <div className="files-content">
      <div className="files-summary">
        <div>
          <strong>{changedFiles.length} ファイルを変更</strong>
          <span className="added">+{changedFiles.reduce((sum, file) => sum + file.added, 0)}</span>
          <span className="removed">
            −{changedFiles.reduce((sum, file) => sum + file.removed, 0)}
          </span>
        </div>
        <Button onClick={() => setUnified(!unified)}>
          {unified ? 'ファイルごとに表示' : 'すべて展開'}
        </Button>
      </div>
      {changedFiles.map((file) => {
        const open = unified || opened.includes(file.path);
        return (
          <section className="file-block" key={file.path}>
            <button
              type="button"
              className="file-heading"
              aria-expanded={open}
              onClick={() => {
                setUnified(false);
                setOpened(
                  open ? opened.filter((path) => path !== file.path) : [...opened, file.path],
                );
              }}
            >
              <Icon name={open ? 'ChevronDown' : 'ChevronRight'} size={12} />
              <span className="truncate">{file.path}</span>
              <small className="added">+{file.added}</small>
              <small className="removed">−{file.removed}</small>
            </button>
            {open && (
              <>
                <div className="diff-meta">
                  <span>変更 1 · {formatTime(session.start + 0.4)}</span>
                  <CopyButton
                    text={diffLines
                      .map(
                        (line) =>
                          `${line.kind === 'added' ? '+' : line.kind === 'removed' ? '-' : ' '}${line.text}`,
                      )
                      .join('\n')}
                  />
                </div>
                <div className="diff-hunk">@@ −427,6 +427,11 @@</div>
                <div className="diff-lines">
                  {diffLines.map((line, index) => (
                    <div className={`diff-line ${line.kind}`} key={index}>
                      <span>{line.kind !== 'added' ? 427 + index : ''}</span>
                      <span>{line.kind !== 'removed' ? 427 + index : ''}</span>
                      <code>
                        {line.kind === 'added' ? '+' : line.kind === 'removed' ? '−' : ' '}{' '}
                        {line.text}
                      </code>
                    </div>
                  ))}
                </div>
              </>
            )}
          </section>
        );
      })}
    </div>
  );
}

function Analysis({
  session,
  onEvent,
}: {
  session: Session;
  onEvent: (event: HistoryEvent) => void;
}) {
  const [query, setQuery] = useState('');
  const [kind, setKind] = useState('all');
  const [status, setStatus] = useState('all');
  const [sort, setSort] = useState('oldest');
  // 指示・応答・思考以外をツールとする（GPUI の is_tool と同じ）
  const isTool = (event: HistoryEvent) => !['user', 'assistant', 'thinking'].includes(event.kind);
  // GPUI の並び替え（src/detail/analysis.rs の indices の比較）と同じ。同順位は元の並び（インデックス昇順）を保つ
  const visible = session.events
    .map((event, index) => ({ event, index }))
    .filter(
      ({ event }) =>
        `${event.name} ${event.summary}`.toLowerCase().includes(query.toLowerCase()) &&
        (kind === 'all' || (kind === 'tools' ? isTool(event) : !isTool(event))) &&
        (status === 'all' ||
          (status === 'failed' ? isFailure(event.status) : event.status === 'pending')),
    )
    .sort((a, b) => {
      const key =
        sort === 'newest'
          ? b.event.start - a.event.start
          : sort === 'duration'
            ? b.event.duration - a.event.duration
            : sort === 'tokens'
              ? b.event.tokens - a.event.tokens
              : a.event.start - b.event.start;
      return key || a.index - b.index;
    })
    .map(({ event }) => event);
  function exportJson() {
    const url = URL.createObjectURL(
      new Blob([JSON.stringify(visible, null, 2)], { type: 'application/json' }),
    );
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = `${session.id}-events.json`;
    anchor.click();
    URL.revokeObjectURL(url);
  }
  return (
    <div className="analysis-content">
      <div className="analysis-heading">
        <Button onClick={exportJson}>JSON を保存</Button>
      </div>
      <Facts
        items={[
          {
            label: 'ツール',
            value: visible.filter(isTool).length,
          },
          {
            label: '失敗・拒否・中断',
            value: visible.filter((event) => isFailure(event.status)).length,
            tone: 'err',
          },
        ]}
      />
      <label className="analysis-search">
        <Icon name="Search" />
        <input
          aria-label="分析のイベントを検索"
          placeholder="名前・要約・モデルを検索"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </label>
      {/* GPUI と同じく、種別・状態の行と並び順の行の 2 行に分ける（src/app/detail/analysis.rs） */}
      <div className="analysis-filters">
        <div className="analysis-filter-row">
          <Segmented
            label="イベント種別"
            options={[
              { value: 'all', label: 'すべて' },
              { value: 'tools', label: 'ツール' },
              { value: 'messages', label: '発話・思考' },
            ]}
            value={kind}
            onChange={setKind}
          />
          <Segmented
            label="イベントの状態"
            options={[
              { value: 'all', label: '全状態' },
              { value: 'failed', label: '失敗・拒否・中断' },
              { value: 'pending', label: '結果なし' },
            ]}
            value={status}
            onChange={setStatus}
          />
        </div>
        <div className="analysis-filter-row">
          <Segmented
            label="並び順"
            options={[
              { value: 'oldest', label: '古い順' },
              { value: 'newest', label: '新しい順' },
              { value: 'duration', label: '時間順' },
              { value: 'tokens', label: 'トークン順' },
            ]}
            value={sort}
            onChange={setSort}
          />
        </div>
      </div>
      <div className="usage-summary">
        <div>
          <span>トークン</span>
          <strong>{formatTokens(session.tokens)}</strong>
        </div>
        <div className="usage-meter">
          <i style={{ width: `${session.usage.input}%` }} />
          <i style={{ width: `${session.usage.cache}%` }} />
          <i style={{ width: `${session.usage.output}%` }} />
        </div>
        <small>
          入力 {session.usage.input}% · キャッシュ {session.usage.cache}% · 出力{' '}
          {session.usage.output}%
        </small>
      </div>
      <Label>
        イベント <span>{visible.length} 件</span>
      </Label>
      {visible.length ? (
        visible.map((event) => (
          <EventRow
            key={event.id}
            event={event}
            query={query}
            live={session.live}
            onClick={() => onEvent(event)}
          />
        ))
      ) : (
        <EmptyState message="該当するイベントはありません" />
      )}
    </div>
  );
}
