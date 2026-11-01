import { useEffect, useState, type ReactNode } from 'react';
import {
  changedFiles,
  diffLines,
  formatDuration,
  formatNumber,
  formatTime,
  formatTokens,
  type DetailTab,
  type HistoryEvent,
  type Session,
} from '../data';
import { eventKinds } from '../tokens';
import {
  Badge,
  Button,
  CopyButton,
  EmptyState,
  Highlight,
  Icon,
  IconButton,
  ProjectMark,
  Segmented,
  SourceLogo,
  Stat,
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
        <span className="detail-heading">セッション詳細</span>
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
          {session.live && <Badge color="var(--live)">進行中</Badge>}
        </div>
        <div className="context-meta">
          <SourceLogo source={session.source} size={12} />
          <span>{session.source === 'claude' ? 'Claude Code' : 'Codex'}</span>
          <span>·</span>
          <ProjectMark name={session.project} size={14} />
          <span>{session.project}</span>
        </div>
      </div>
      <div className="detail-scope">
        <Icon
          name={tab === 'overview' ? 'Layers' : event ? eventKinds[event.kind].icon : 'ListTree'}
          size={13}
        />
        <span>{tab === 'overview' ? 'セッション全体' : event ? event.name : 'メインのレーン'}</span>
        <small>
          {formatTime(session.start)}–{formatTime(session.end)}
        </small>
      </div>
      <div className="detail-tabs" role="tablist" aria-label="詳細のタブ">
        {(
          [
            ['overview', '概要', null],
            ['history', '履歴', session.events.length],
            ['files', '変更', changedFiles.length],
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
        {tab === 'files' && <Files />}
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

function Label({ children }: { children: ReactNode }) {
  return <h3 className="detail-label">{children}</h3>;
}

function Overview({
  session,
  onTab,
  onEvent,
}: Pick<DetailPanelProps, 'session' | 'onTab' | 'onEvent'>) {
  const failures = session.events.filter((event) => event.status !== 'success').length;
  const turns = session.events.filter((event) => event.kind === 'user');
  return (
    <div className="overview-content">
      <Label>セッションの集計</Label>
      <div className="stat-row">
        <Stat label="経過時間" value={formatDuration(session.end - session.start)} />
        <Stat label="ターン" value={turns.length} />
        <Stat label="失敗等" value={failures} color={failures ? 'var(--err)' : undefined} />
      </div>
      <div className="stat-row">
        <Stat label="API換算料金" value={`$${session.cost.toFixed(2)}`} />
        <Stat label="キャッシュヒット率" value="84.2%" />
      </div>
      <div className="detail-actions">
        <CopyButton
          label="再開コマンドをコピー"
          text={`${session.source === 'claude' ? 'claude --resume' : 'codex resume'} ${session.id}`}
        />
        <Button
          icon="Layers"
          onClick={() => {
            onEvent(null);
            onTab('history');
          }}
        >
          メインの履歴
        </Button>
      </div>
      <Label>
        指示と応答 <span>メイン</span>
      </Label>
      {turns.map((turn, i) => (
        <button
          type="button"
          className="turn-card"
          key={turn.id}
          onClick={() => {
            onEvent(turn);
            onTab('history');
          }}
        >
          <div className="turn-meta">
            指示 {i + 1} · {formatTime(turn.start)}
            <Icon name="ChevronRight" size={12} />
          </div>
          <strong>{i === 0 ? session.title : turn.summary}</strong>
          <p>{i === 0 ? turn.input : '変更を確認して、テストを実行してください。'}</p>
          <div className="turn-response">
            <Icon name="MessageSquare" size={12} />
            <span>
              {i === 0
                ? '既存の実装を確認し、変更とテストを進めます。'
                : '実装とテストの確認が完了しました。'}
            </span>
          </div>
        </button>
      ))}
      <Label>エージェント</Label>
      <button
        type="button"
        className="agent-card"
        onClick={() => {
          onEvent(null);
          onTab('history');
        }}
      >
        <Icon name="GitBranch" />
        <span>メイン</span>
        <small>{session.events.length} 件</small>
        <Icon name="ChevronRight" size={12} />
      </button>
      {session.agents.map((agent) => (
        <button
          type="button"
          className="agent-card subagent"
          key={agent}
          onClick={() => {
            onEvent(session.events.find((event) => event.kind === 'agent') ?? null);
            onTab('history');
          }}
        >
          <Icon name="Bot" />
          <span>{agent}</span>
          <small>6 件</small>
          <Icon name="ChevronRight" size={12} />
        </button>
      ))}
      <Label>セッション情報</Label>
      <dl className="session-info">
        <dt>開始</dt>
        <dd>
          2026/10/{26 + session.day} {formatTime(session.start)}
        </dd>
        <dt>最終更新</dt>
        <dd>{formatTime(session.end)}</dd>
        <dt>作業ディレクトリ</dt>
        <dd>~/projects/{session.project}</dd>
        <dt>モデル</dt>
        <dd>{session.source === 'claude' ? 'claude-sonnet-4-5' : 'gpt-5-codex'}</dd>
        <dt>トークン</dt>
        <dd>{formatNumber(session.tokens)}</dd>
        <dt>セッション ID</dt>
        <dd>
          <code>{session.id}</code>
        </dd>
      </dl>
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
  const visible = session.events.filter((event) => !failedOnly || event.status !== 'success');
  if (reversed) visible.reverse();
  return (
    <div className="history-content">
      <div className="history-toolbar">
        <strong>{event ? event.name : 'イベントの履歴'}</strong>
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
            <Icon name="Clock" size={12} />
            10/{26 + session.day} {formatTime(event.start)}
            <span>·</span>
            {Math.round(event.duration * 3600)}秒
            <Badge color={event.status === 'success' ? 'var(--ok)' : 'var(--err)'}>
              {event.status === 'success' ? '成功' : event.status === 'failed' ? '失敗' : '中断'}
            </Badge>
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
            icon="ListTree"
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
              onClick={() => setFailedOnly(!failedOnly)}
              className={failedOnly ? 'active' : ''}
            >
              <Icon name="CircleAlert" size={12} />
              失敗等のみ
            </button>
            <button type="button" aria-pressed={reversed} onClick={() => setReversed(!reversed)}>
              <Icon name={reversed ? 'ArrowUp' : 'ArrowDown'} size={12} />
              {reversed ? '新しい順' : '古い順'}
            </button>
          </div>
          {visible.length ? (
            visible.map((item) => (
              <EventRow event={item} key={item.id} onClick={() => onEvent(item)} />
            ))
          ) : (
            <EmptyState filtered />
          )}
        </>
      )}
    </div>
  );
}

export function EventRow({
  event,
  query = '',
  onClick,
}: {
  event: HistoryEvent;
  query?: string;
  onClick: () => void;
}) {
  const token = eventKinds[event.kind];
  return (
    <button type="button" className="event-row" onClick={onClick}>
      <span className="event-row-icon" style={{ color: token.color }}>
        <Icon name={token.icon} />
      </span>
      <span className="row-content">
        <span className="event-row-title">
          <strong>
            <Highlight text={event.name} query={query} />
          </strong>
          {event.status !== 'success' && (
            <Badge color="var(--err)">{event.status === 'failed' ? '失敗' : '中断'}</Badge>
          )}
          <small>{formatTime(event.start)}</small>
        </span>
        <span className="truncate">
          <Highlight text={event.summary} query={query} />
        </span>
      </span>
      <Icon name="ChevronRight" size={12} />
    </button>
  );
}

function Files() {
  const [opened, setOpened] = useState<string[]>([changedFiles[0].path]);
  const [unified, setUnified] = useState(false);
  return (
    <div className="files-content">
      <div className="files-summary">
        <div>
          <strong>{changedFiles.length} ファイルを変更</strong>
          <span className="added">+55</span>
          <span className="removed">−19</span>
        </div>
        <Button icon="FilePen" onClick={() => setUnified(!unified)}>
          {unified ? 'ファイルごとに表示' : 'すべて展開'}
        </Button>
      </div>
      {changedFiles.map((file) => {
        const open = unified || opened.includes(file.path);
        return (
          <section className="file-card" key={file.path}>
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
              <Icon name="FilePen" size={13} />
              <span className="truncate">{file.path}</span>
              <small className="added">+{file.added}</small>
              <small className="removed">−{file.removed}</small>
            </button>
            {open && (
              <>
                <div className="diff-meta">
                  <span>変更 1 · 09:42</span>
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
  const visible = session.events.filter(
    (event) =>
      `${event.name} ${event.summary}`.toLowerCase().includes(query.toLowerCase()) &&
      (kind === 'all' ||
        (kind === 'tools'
          ? !['user', 'assistant', 'thinking'].includes(event.kind)
          : event.kind === kind)) &&
      (status === 'all' || event.status !== 'success'),
  );
  if (sort === 'newest') visible.reverse();
  if (sort === 'duration') visible.sort((a, b) => b.duration - a.duration);
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
        <Label>選択範囲の分析</Label>
        <Button icon="Copy" onClick={exportJson}>
          JSON を保存
        </Button>
      </div>
      <div className="stat-row">
        <Stat label="イベント" value={visible.length} />
        <Stat
          label="ツール"
          value={
            visible.filter((event) => !['user', 'assistant', 'thinking'].includes(event.kind))
              .length
          }
        />
        <Stat
          label="失敗等"
          value={visible.filter((event) => event.status !== 'success').length}
          color="var(--err)"
        />
      </div>
      <label className="analysis-search">
        <Icon name="Search" />
        <input
          aria-label="分析のイベントを検索"
          placeholder="名前・要約・モデルを検索"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </label>
      <div className="analysis-filters">
        <Segmented
          label="イベント種別"
          options={[
            { value: 'all', label: 'すべて' },
            { value: 'tools', label: 'ツール' },
            { value: 'user', label: '指示' },
          ]}
          value={kind}
          onChange={setKind}
        />
        <Segmented
          label="イベントの状態"
          options={[
            { value: 'all', label: 'すべて' },
            { value: 'failed', label: '失敗等' },
          ]}
          value={status}
          onChange={setStatus}
        />
        <select
          aria-label="イベントの並び順"
          value={sort}
          onChange={(event) => setSort(event.target.value)}
        >
          <option value="oldest">古い順</option>
          <option value="newest">新しい順</option>
          <option value="duration">所要時間</option>
        </select>
      </div>
      <div className="usage-summary">
        <div>
          <span>トークン</span>
          <strong>{formatTokens(session.tokens)}</strong>
        </div>
        <div className="usage-meter">
          <i style={{ width: '18%' }} />
          <i style={{ width: '68%' }} />
          <i style={{ width: '14%' }} />
        </div>
        <small>入力 18% · キャッシュ 68% · 出力 14%</small>
      </div>
      <Label>
        イベント <span>{visible.length} 件</span>
      </Label>
      {visible.length ? (
        visible.map((event) => (
          <EventRow key={event.id} event={event} query={query} onClick={() => onEvent(event)} />
        ))
      ) : (
        <EmptyState filtered />
      )}
    </div>
  );
}
