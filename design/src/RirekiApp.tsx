import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent,
} from 'react';
import {
  filterSessions,
  formatNumber,
  formatTime,
  formatDate as dateText,
  projectColor,
  projects,
  sessions as sampleSessions,
  type DetailTab,
  type HistoryEvent,
  type Overlay,
  type Session,
  type Source,
  type View,
} from './data';
import { eventGroups } from './tokens';
import { Sidebar } from './components/Sidebar';
import { Minimap, Timeline, type Granularity, type TimeRange } from './components/Timeline';
import { CalendarView } from './components/Calendar';
import { DetailPanel } from './components/DetailPanel';
import { Statistics } from './components/Statistics';
import { DisplayOptions, HelpDialog, McpDialog, RangePicker } from './components/Overlays';
import {
  Button,
  EmptyState,
  Icon,
  IconButton,
  Segmented,
  SourceLogo,
} from './components/primitives';

export interface RirekiAppProps {
  initialView?: View;
  initialSource?: Source;
  initialProject?: string | null;
  initialSelection?: string | null;
  initialEvent?: number | null;
  initialTab?: DetailTab;
  initialOverlay?: Overlay;
  initialCost?: boolean;
  initialDirectory?: boolean;
  initialSidebar?: boolean;
  initialStatus?: 'ready' | 'loading' | 'empty';
  initialQuery?: string;
  initialMcpRunning?: boolean;
  initialFullscreen?: boolean;
}

export function RirekiApp({
  initialView = 'timeline',
  initialSource = 'all',
  initialProject = null,
  initialSelection = null,
  initialEvent = null,
  initialTab = 'overview',
  initialOverlay = null,
  initialCost = false,
  initialDirectory = false,
  initialSidebar = true,
  initialStatus = 'ready',
  initialQuery = '',
  initialMcpRunning = false,
  initialFullscreen = false,
}: RirekiAppProps) {
  const [view, setView] = useState(initialView);
  const [calendarMode, setCalendarMode] = useState<View>(
    initialView === 'timeline' || initialView === 'statistics' ? 'week' : initialView,
  );
  const [source, setSource] = useState(initialSource);
  const [project, setProject] = useState(initialProject);
  const [query, setQuery] = useState(initialQuery);
  const [selected, setSelected] = useState(initialSelection);
  const [eventId, setEventId] = useState(
    initialSelection && initialEvent !== null
      ? (sampleSessions.find((session) => session.id === initialSelection)?.events[initialEvent]
          ?.id ?? null)
      : null,
  );
  const [tab, setTab] = useState(initialTab);
  const [overlay, setOverlay] = useState(initialOverlay);
  const [cost, setCost] = useState(initialCost);
  const [directory, setDirectory] = useState(initialDirectory);
  const [sidebar, setSidebar] = useState(initialSidebar);
  const [fullscreen, setFullscreen] = useState(initialFullscreen);
  const [mcpRunning, setMcpRunning] = useState(initialMcpRunning);
  const [range, setRange] = useState<TimeRange>({ start: 8, end: 18 });
  const [granularity, setGranularity] = useState<Granularity>('auto');
  const [inViewOnly, setInViewOnly] = useState(false);
  const [offset, setOffset] = useState(0);
  const [day, setDay] = useState(0);
  const [sidebarWidth, setSidebarWidth] = useState(260);
  const [detailWidth, setDetailWidth] = useState(440);
  const app = useRef<HTMLDivElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const drag = useRef<{ kind: 'sidebar' | 'detail'; x: number; width: number } | null>(null);
  const loading = initialStatus === 'loading';
  const empty = initialStatus !== 'ready';
  const scoped = empty ? [] : filterSessions(source, project, query);
  // タイムラインの表示範囲に作業がないとき（絞り込み後の履歴はある）。本体 timeline.rs の has_any に対応する
  const periodEmpty =
    view === 'timeline' &&
    scoped.length > 0 &&
    !scoped.some(
      (session) =>
        (session.day - day) * 24 + session.end > range.start &&
        (session.day - day) * 24 + session.start < range.end,
    );
  const session = selected
    ? (sampleSessions.find((session) => session.id === selected) ?? null)
    : null;
  const event = session?.events.find((event) => event.id === eventId) ?? null;
  const showDetail = session && view !== 'statistics';
  const isFullscreen = !!showDetail && fullscreen;
  const closeOverlay = useCallback(() => setOverlay(null), []);

  useEffect(() => {
    app.current?.focus({ preventScroll: true });
  }, []);

  function selectSession(session: Session, event?: HistoryEvent) {
    setSelected(session.id);
    setEventId(event?.id ?? null);
    setTab(event ? 'history' : 'overview');
  }
  function changeView(next: View) {
    setView(next);
    setOverlay(null);
    setFullscreen(false);
    setOffset(0);
    if (next !== 'timeline' && next !== 'statistics') setCalendarMode(next);
  }
  function openDay(next: number) {
    setDay(next);
    setOffset(0);
    changeView('day');
  }
  function focusSearch() {
    setSidebar(true);
    requestAnimationFrame(() => searchInput.current?.focus());
  }
  function movePeriod(direction: number) {
    if (view === 'timeline') {
      const span = range.end - range.start;
      setRange({
        start: range.start + span * direction * 0.7,
        end: range.end + span * direction * 0.7,
      });
    } else setOffset((value) => value + direction);
  }
  function zoom(factor: number) {
    const center = (range.start + range.end) / 2;
    const half = Math.max(0.05, Math.min(36, ((range.end - range.start) / 2) * factor));
    setRange({ start: center - half, end: center + half });
  }
  function today() {
    setDay(0);
    setOffset(0);
    setRange({ start: 8, end: 18 });
  }
  function keydown(e: KeyboardEvent<HTMLDivElement>) {
    const command = e.metaKey || e.ctrlKey;
    const editable = (e.target as HTMLElement).matches('input, textarea, select');
    if (e.key === 'Escape') {
      if (overlay) setOverlay(null);
      else if (fullscreen) setFullscreen(false);
      else {
        setSelected(null);
        setEventId(null);
      }
      return;
    }
    if (command && ['k', 'f'].includes(e.key.toLowerCase())) {
      e.preventDefault();
      focusSearch();
      return;
    }
    if (editable || overlay === 'mcp' || overlay === 'help') return;
    if (command && ['1', '2', '3', '4'].includes(e.key)) {
      e.preventDefault();
      changeView(
        ({ '1': 'timeline', '2': 'day', '3': 'week', '4': 'statistics' } as const)[e.key as '1'],
      );
    } else if (command && e.key.toLowerCase() === 'b') {
      e.preventDefault();
      setSidebar(!sidebar);
    } else if (command && e.shiftKey && e.key === 'Enter' && showDetail) {
      e.preventDefault();
      setFullscreen(!fullscreen);
    } else if (!command) {
      if (e.key === '?') setOverlay('help');
      if (e.key.toLowerCase() === 'g') setOverlay(overlay === 'range' ? null : 'range');
      if (e.key.toLowerCase() === 'd') changeView('day');
      if (e.key.toLowerCase() === 'w') changeView('week');
      if (e.key.toLowerCase() === 'm') changeView('month');
      if (e.key.toLowerCase() === 't') today();
      if (e.key.toLowerCase() === 'v') setInViewOnly(!inViewOnly);
      if (e.key === '+' || e.key === '=') zoom(0.7);
      if (e.key === '-') zoom(1.4);
      if (e.key === 'ArrowLeft') {
        e.preventDefault();
        movePeriod(-1);
      }
      if (e.key === 'ArrowRight') {
        e.preventDefault();
        movePeriod(1);
      }
      if (showDetail && ['1', '2', '3', '4'].includes(e.key))
        setTab((['overview', 'history', 'files', 'analysis'] as const)[Number(e.key) - 1]);
      if (session && ['j', 'k'].includes(e.key.toLowerCase())) {
        const index = session.events.findIndex((event) => event.id === eventId);
        const next =
          session.events[
            Math.min(
              session.events.length - 1,
              Math.max(0, index + (e.key.toLowerCase() === 'j' ? 1 : -1)),
            )
          ];
        setEventId(next.id);
        setTab('history');
      }
    }
  }
  function resizeStart(e: PointerEvent<HTMLDivElement>, kind: 'sidebar' | 'detail') {
    drag.current = { kind, x: e.clientX, width: kind === 'sidebar' ? sidebarWidth : detailWidth };
    e.currentTarget.setPointerCapture(e.pointerId);
  }
  function resizeMove(e: PointerEvent<HTMLDivElement>) {
    if (!drag.current) return;
    const { kind, x, width } = drag.current;
    const delta = e.clientX - x;
    if (kind === 'sidebar') setSidebarWidth(Math.min(480, Math.max(200, width + delta)));
    else setDetailWidth(Math.min(800, Math.max(320, width - delta)));
  }
  const resizeHandle = (kind: 'sidebar' | 'detail') => (
    <div
      role="separator"
      aria-label={kind === 'sidebar' ? 'サイドバーの幅' : '詳細パネルの幅'}
      aria-orientation="vertical"
      aria-valuemin={kind === 'sidebar' ? 200 : 320}
      aria-valuemax={kind === 'sidebar' ? 480 : 800}
      aria-valuenow={kind === 'sidebar' ? sidebarWidth : detailWidth}
      tabIndex={0}
      className={`pane-resizer ${kind}-resizer`}
      onPointerDown={(e) => resizeStart(e, kind)}
      onPointerMove={resizeMove}
      onPointerUp={() => {
        drag.current = null;
      }}
      onPointerCancel={() => {
        drag.current = null;
      }}
      onKeyDown={(e) => {
        if (!['ArrowLeft', 'ArrowRight'].includes(e.key)) return;
        e.preventDefault();
        e.stopPropagation();
        const delta = e.key === 'ArrowRight' ? 20 : -20;
        if (kind === 'sidebar')
          setSidebarWidth((value) => Math.max(200, Math.min(480, value + delta)));
        else setDetailWidth((value) => Math.max(320, Math.min(800, value - delta)));
      }}
    />
  );
  const timelineStartDay = day + Math.floor(range.start / 24);
  const timelineEndDay = day + Math.floor(range.end / 24);
  const title =
    view === 'timeline'
      ? `${dateText(timelineStartDay)} ${formatTime(range.start)}–${timelineStartDay === timelineEndDay ? '' : `${dateText(timelineEndDay, true)} `}${formatTime(range.end)}`
      : view === 'month'
        ? `${new Date(Date.UTC(2026, 9 + offset, 1)).getUTCFullYear()}年${new Date(Date.UTC(2026, 9 + offset, 1)).getUTCMonth() + 1}月`
        : view === 'day'
          ? dateText(day + offset)
          : `${dateText(offset * 7)}–${dateText(offset * 7 + 6, true)}`;

  return (
    <div
      ref={app}
      tabIndex={-1}
      className={`rireki-app ${showDetail ? 'has-detail' : ''} ${isFullscreen ? 'detail-fullscreen' : ''}`}
      onKeyDown={keydown}
      style={
        { '--sidebar-w': `${sidebarWidth}px`, '--detail-w': `${detailWidth}px` } as CSSProperties
      }
    >
      <div className="title-strip">
        <div className="traffic-lights" aria-hidden="true">
          <i />
          <i />
          <i />
        </div>
        <strong>rireki</strong>
        {loading && (
          <>
            <span>·</span>
            <Icon name="LoaderCircle" size={12} className="spin" />
            <span>索引を更新中 8/24 ファイル</span>
          </>
        )}
      </div>
      <div className="app-body">
        {!isFullscreen && (
          <nav className="rail" aria-label="メインナビゲーション">
            <div className="source-tiles">
              {(['all', 'claude', 'codex'] as const).map((value) => (
                <button
                  type="button"
                  key={value}
                  className={`source-tile ${value} ${source === value ? 'selected' : ''}`}
                  aria-label={
                    value === 'all'
                      ? 'すべての履歴'
                      : value === 'claude'
                        ? 'Claude Code の履歴'
                        : 'Codex の履歴'
                  }
                  aria-pressed={source === value}
                  title={
                    value === 'all'
                      ? 'すべての履歴'
                      : value === 'claude'
                        ? 'Claude Code の履歴'
                        : 'Codex の履歴'
                  }
                  onClick={() => {
                    setSource(value);
                    setSelected(null);
                  }}
                >
                  <span>
                    <SourceLogo source={value} size={value === 'all' ? 44 : 18} />
                  </span>
                  {!empty &&
                    sampleSessions.some(
                      (session) => session.live && (value === 'all' || session.source === value),
                    ) && <i className="live-dot" />}
                </button>
              ))}
            </div>
            <div className="rail-divider" />
            {(
              [
                { value: 'timeline', icon: 'ListTree', label: 'タイムライン' },
                { value: calendarMode, icon: 'Calendar', label: 'カレンダー' },
                { value: 'statistics', icon: 'ChartColumn', label: '統計' },
              ] as const
            ).map((item) => (
              <IconButton
                key={item.label}
                icon={item.icon}
                label={item.label}
                active={
                  item.label === 'カレンダー'
                    ? !['timeline', 'statistics'].includes(view)
                    : view === item.value
                }
                aria-pressed={
                  item.label === 'カレンダー'
                    ? !['timeline', 'statistics'].includes(view)
                    : view === item.value
                }
                onClick={() => changeView(item.value)}
              />
            ))}
            <div className="rail-spacer" />
            <span className="rail-mcp">
              <IconButton
                icon="Plug"
                label="MCP サーバー"
                active={overlay === 'mcp'}
                onClick={() => setOverlay(overlay === 'mcp' ? null : 'mcp')}
              />
              {mcpRunning && <i className="live-dot" />}
            </span>
            <IconButton
              icon="Keyboard"
              label="キー操作"
              active={overlay === 'help'}
              onClick={() => setOverlay(overlay === 'help' ? null : 'help')}
            />
          </nav>
        )}
        <div className={`workspace ${view === 'statistics' ? 'statistics-workspace' : ''}`}>
          {sidebar && view !== 'statistics' && !isFullscreen && (
            <div className="sidebar-container">
              <Sidebar
                source={source}
                project={project}
                query={query}
                selected={selected}
                empty={empty}
                inputRef={searchInput}
                onQuery={setQuery}
                onProject={(value) => {
                  setProject(value);
                  setSelected(null);
                }}
                onSession={(session, event) => {
                  selectSession(session, event);
                  setDay(session.day);
                  setRange({ start: Math.floor(session.start), end: Math.ceil(session.end) });
                }}
                onClose={() => setSidebar(false)}
              />
              {resizeHandle('sidebar')}
            </div>
          )}
          {!isFullscreen &&
            (view === 'statistics' ? (
              <Statistics
                sessions={loading ? filterSessions(source, project) : scoped}
                project={project}
                loading={loading}
                onProject={setProject}
                onSession={(session) => {
                  changeView('timeline');
                  selectSession(session);
                  setDay(session.day);
                  setRange({ start: Math.floor(session.start), end: Math.ceil(session.end) });
                }}
              />
            ) : (
              <main className="workspace-main">
                <header className="toolbar">
                  {!sidebar && (
                    <IconButton
                      icon="PanelLeftOpen"
                      label="サイドバーを表示"
                      onClick={() => setSidebar(true)}
                    />
                  )}
                  <div className="period-navigation">
                    <IconButton
                      icon="ChevronLeft"
                      label="前の期間"
                      onClick={() => movePeriod(-1)}
                    />
                    <Button onClick={today}>今日</Button>
                    <IconButton
                      icon="ChevronRight"
                      label="次の期間"
                      onClick={() => movePeriod(1)}
                    />
                  </div>
                  <Button
                    icon="Calendar"
                    className="period-button"
                    aria-expanded={overlay === 'range'}
                    onClick={() => setOverlay(overlay === 'range' ? null : 'range')}
                  >
                    {title}
                    <Icon name="ChevronDown" size={11} />
                  </Button>
                  <div className="toolbar-spacer" />
                  {project && (
                    <Button icon="Folder" className="focus-chip" onClick={() => setProject(null)}>
                      {project}
                      <Icon name="X" size={10} />
                    </Button>
                  )}
                  {view === 'timeline' ? (
                    <>
                      <div className="zoom-controls">
                        <IconButton icon="ZoomOut" label="縮小" onClick={() => zoom(1.4)} />
                        <IconButton icon="ZoomIn" label="拡大" onClick={() => zoom(0.7)} />
                      </div>
                      <div className="metric-switch">
                        <Segmented
                          label="タイムラインの表示内容"
                          toolbar
                          options={[
                            { value: 'events', label: 'イベント' },
                            { value: 'cost', label: '料金' },
                          ]}
                          value={cost ? 'cost' : 'events'}
                          onChange={(value) => setCost(value === 'cost')}
                        />
                      </div>
                      <Button
                        icon="SlidersHorizontal"
                        className={`display-button ${inViewOnly ? 'filtered' : ''}`}
                        title={
                          inViewOnly
                            ? '表示オプション（表示範囲内のセッションだけ）'
                            : '表示オプション'
                        }
                        aria-expanded={overlay === 'display'}
                        onClick={() => setOverlay(overlay === 'display' ? null : 'display')}
                      >
                        <span>表示</span>
                        <Icon name="ChevronDown" size={10} />
                      </Button>
                    </>
                  ) : (
                    <Segmented
                      label="カレンダーの表示"
                      toolbar
                      options={[
                        { value: 'day', label: '日' },
                        { value: 'week', label: '週' },
                        { value: 'month', label: '月' },
                      ]}
                      value={view}
                      onChange={changeView}
                    />
                  )}
                </header>
                {view === 'timeline' && (
                  <Minimap
                    sessions={scoped.filter((session) => session.day === day)}
                    range={range}
                    onRange={setRange}
                  />
                )}
                <div className="main-view">
                  {!scoped.length || loading || periodEmpty ? (
                    <EmptyState
                      message={
                        loading
                          ? '読み込み中…'
                          : periodEmpty
                            ? 'この期間の作業履歴はありません'
                            : '表示できる履歴がありません'
                      }
                      actionLabel={periodEmpty && !loading ? '最新の作業へ移動' : undefined}
                      onAction={today}
                    />
                  ) : view === 'timeline' ? (
                    <Timeline
                      sessions={scoped}
                      range={range}
                      day={day}
                      selected={selected}
                      selectedEvent={eventId}
                      cost={cost}
                      directory={directory}
                      granularity={granularity}
                      inViewOnly={inViewOnly}
                      onSelect={selectSession}
                    />
                  ) : (
                    <CalendarView
                      mode={view}
                      sessions={scoped}
                      offset={offset}
                      selectedDay={day}
                      onDay={openDay}
                      onSelect={selectSession}
                    />
                  )}
                </div>
                <footer className="footer">
                  <span>
                    {scoped.length} セッション
                    {view === 'timeline' &&
                      ` · ${formatNumber(scoped.reduce((sum, session) => sum + session.events.length, 0))} イベント · 目盛り ${Math.round((range.end - range.start) * 6)}分 / 100px`}
                  </span>
                  <div className="footer-legend">
                    {view === 'timeline' ? (
                      cost ? (
                        <>
                          <span>
                            <i className="claude-color" />
                            Claude Code
                          </span>
                          <span>
                            <i className="codex-color" />
                            Codex
                          </span>
                          <span>料金（USD）</span>
                        </>
                      ) : (
                        Object.entries(eventGroups)
                          .filter(([group]) => group !== 'thinking')
                          .map(([group, token]) => (
                            <span key={group}>
                              <i style={{ background: token.color }} />
                              {token.label}
                            </span>
                          ))
                      )
                    ) : (
                      projects
                        .filter((project) =>
                          scoped.some((session) => session.project === project.name),
                        )
                        .map((project) => (
                          <span key={project.name}>
                            <i style={{ background: projectColor(project.name) }} />
                            {project.name}
                          </span>
                        ))
                    )}
                  </div>
                </footer>
              </main>
            ))}
          {showDetail && (
            <div className={`detail-container ${isFullscreen ? 'fullscreen' : ''}`}>
              <DetailPanel
                session={session}
                event={event}
                tab={tab}
                fullscreen={isFullscreen}
                onTab={setTab}
                onEvent={(event) => setEventId(event?.id ?? null)}
                onClose={() => {
                  setSelected(null);
                  setFullscreen(false);
                }}
                onFullscreen={() => setFullscreen(!fullscreen)}
                onLocate={() => {
                  setFullscreen(false);
                  setDay(session.day);
                  setRange({ start: Math.floor(session.start), end: Math.ceil(session.end) });
                }}
              />
              {!isFullscreen && resizeHandle('detail')}
            </div>
          )}
        </div>
      </div>
      {overlay === 'display' && (
        <DisplayOptions
          cost={cost}
          directory={directory}
          granularity={granularity}
          inViewOnly={inViewOnly}
          onCost={setCost}
          onDirectory={setDirectory}
          onGranularity={setGranularity}
          onInViewOnly={setInViewOnly}
          onClose={closeOverlay}
        />
      )}
      {overlay === 'range' && (
        <RangePicker
          sessions={scoped}
          onRange={setRange}
          onDay={(value) => {
            setDay(value);
            setOffset(0);
          }}
          onClose={closeOverlay}
        />
      )}
      {overlay === 'help' && <HelpDialog onClose={closeOverlay} />}
      {overlay === 'mcp' && (
        <McpDialog running={mcpRunning} onRunning={setMcpRunning} onClose={closeOverlay} />
      )}
    </div>
  );
}
