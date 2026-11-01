import { useState, type ReactNode, type RefObject } from 'react';
import {
  filterSessions,
  formatTime,
  projects,
  type HistoryEvent,
  type Session,
  type Source,
} from '../data';
import { Highlight, Icon, IconButton, ProjectMark, SourceLogo } from './primitives';

function Section({
  title,
  count,
  children,
}: {
  title: string;
  count: number;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(true);
  return (
    <section>
      <button
        className="section-heading"
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <Icon name={open ? 'ChevronDown' : 'ChevronRight'} size={12} />
        {title}
        <span>{count}</span>
      </button>
      {open && children}
    </section>
  );
}

export interface SidebarProps {
  source: Source;
  project: string | null;
  query: string;
  selected: string | null;
  empty?: boolean;
  inputRef?: RefObject<HTMLInputElement | null>;
  onQuery: (query: string) => void;
  onProject: (project: string | null) => void;
  onSession: (session: Session, event?: HistoryEvent) => void;
  onClose: () => void;
}

export function Sidebar({
  source,
  project,
  query,
  selected,
  empty,
  inputRef,
  onQuery,
  onProject,
  onSession,
  onClose,
}: SidebarProps) {
  const scoped = empty ? [] : filterSessions(source, null);
  const visible = empty ? [] : filterSessions(source, project, query);
  const live = visible.filter((session) => session.live);
  const searchEvents = query
    ? visible
        .flatMap((session) =>
          session.events
            .filter((event) =>
              `${event.name} ${event.summary}`.toLowerCase().includes(query.toLowerCase()),
            )
            .slice(0, 3)
            .map((event) => ({ session, event })),
        )
        .slice(0, 12)
    : [];
  const placeholder = project
    ? `${project} を検索`
    : source === 'all'
      ? 'すべての履歴を検索'
      : `${source === 'claude' ? 'Claude Code' : 'Codex'} の履歴を検索`;
  const sessionRow = (session: Session, isLive = false) => (
    <button
      type="button"
      key={session.id}
      className={`sidebar-row session-row ${selected === session.id ? 'selected' : ''} ${isLive ? 'live-row' : ''}`}
      onClick={() => onSession(session)}
      title={session.title}
    >
      {isLive ? <span className="live-dot" /> : <SourceLogo source={session.source} />}
      <span className="row-content">
        <span className="truncate">
          <Highlight text={session.title} query={query} />
        </span>
        {isLive && (
          <span className="row-meta">
            <SourceLogo source={session.source} size={11} />
            {session.project}
            <span>·</span>たった今
          </span>
        )}
      </span>
      {!isLive && (
        <span className="row-time">
          {session.live ? <span className="live-dot" /> : formatTime(session.start)}
        </span>
      )}
    </button>
  );
  return (
    <aside className="sidebar" aria-label="履歴ナビゲーション">
      <div className="search-header">
        <label className="search-input">
          <Icon name="Search" />
          <input
            ref={inputRef}
            aria-label="履歴を検索"
            placeholder={placeholder}
            value={query}
            onChange={(event) => onQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                onQuery('');
                event.stopPropagation();
              }
              if (event.key === 'Enter' && visible[0]) onSession(visible[0]);
            }}
          />
          {query ? (
            <IconButton icon="X" label="検索を解除" onClick={() => onQuery('')} />
          ) : (
            <kbd>⌘K</kbd>
          )}
        </label>
        <IconButton icon="PanelLeftClose" label="サイドバーを隠す" onClick={onClose} />
      </div>
      <div className="sidebar-scroll">
        {query ? (
          <>
            <div className="search-summary">
              <Icon name="Search" size={12} />
              <span>「{query}」の検索結果</span>
              <span>{visible.length + searchEvents.length}</span>
            </div>
            <Section title="セッション" count={visible.length}>
              {visible.map((session) => sessionRow(session))}
            </Section>
            {searchEvents.length > 0 && (
              <Section title="イベント" count={searchEvents.length}>
                {searchEvents.map(({ session, event }) => (
                  <button
                    type="button"
                    className="sidebar-row search-event"
                    key={event.id}
                    onClick={() => onSession(session, event)}
                  >
                    <Icon name="FileText" />
                    <span className="row-content">
                      <strong>
                        <Highlight text={event.name} query={query} />
                      </strong>
                      <span className="truncate">
                        <Highlight text={event.summary} query={query} />
                      </span>
                      <small>
                        {session.project} · {session.title}
                      </small>
                    </span>
                  </button>
                ))}
              </Section>
            )}
            {!visible.length && <div className="sidebar-no-results">一致する履歴はありません</div>}
          </>
        ) : (
          <>
            {!!live.length && (
              <Section title="進行中" count={live.length}>
                {live.map((session) => sessionRow(session, true))}
              </Section>
            )}
            <Section
              title="プロジェクト"
              count={projects.filter((p) => scoped.some((s) => s.project === p.name)).length}
            >
              <button
                type="button"
                className={`sidebar-row ${project === null ? 'selected' : ''}`}
                onClick={() => onProject(null)}
              >
                <Icon name="Layers" size={18} />
                <span className="row-content truncate">すべてのプロジェクト</span>
                <span className="count">{scoped.length}</span>
              </button>
              {projects
                .filter((p) => scoped.some((s) => s.project === p.name))
                .map((p) => (
                  <button
                    key={p.name}
                    type="button"
                    className={`sidebar-row ${project === p.name ? 'selected' : ''}`}
                    onClick={() => onProject(project === p.name ? null : p.name)}
                    title={`~/projects/${p.name}`}
                  >
                    <span className="project-with-status">
                      <ProjectMark name={p.name} />
                      {scoped.some((s) => s.project === p.name && s.live) && (
                        <span className="live-dot" />
                      )}
                    </span>
                    <span className="row-content truncate">{p.name}</span>
                    <span className="count">
                      {scoped.filter((s) => s.project === p.name).length}
                    </span>
                  </button>
                ))}
            </Section>
            <Section
              title={project ? `${project} のセッション` : '最近のセッション'}
              count={visible.length}
            >
              {[...visible].reverse().map((session) => sessionRow(session))}
            </Section>
          </>
        )}
      </div>
    </aside>
  );
}
