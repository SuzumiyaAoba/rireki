import { useEffect, useRef, useState, type CSSProperties } from 'react';
import {
  formatDate,
  formatNumber,
  formatTime,
  projectColor,
  type HistoryEvent,
  type Session,
} from '../data';
import { EventBlock, Icon, ProjectMark, SourceLogo } from './primitives';

export type Granularity = 'auto' | 'same-tool' | 'turn' | 'call';
export interface TimeRange {
  start: number;
  end: number;
}

interface TimelineProps {
  sessions: Session[];
  range: TimeRange;
  day: number;
  selected: string | null;
  selectedEvent: string | null;
  cost: boolean;
  directory: boolean;
  granularity: Granularity;
  inViewOnly: boolean;
  onSelect: (session: Session, event?: HistoryEvent) => void;
}

function aggregate(events: HistoryEvent[], granularity: Granularity, span: number) {
  const groups: HistoryEvent[][] = [];
  for (const event of events) {
    const last = groups.at(-1);
    const shouldJoin =
      last &&
      (granularity === 'turn'
        ? event.kind !== 'user'
        : granularity === 'same-tool'
          ? last.at(-1)?.name === event.name
          : granularity === 'auto' && span > 12 && last.at(-1)?.kind === event.kind);
    if (shouldJoin) last.push(event);
    else groups.push([event]);
  }
  return groups;
}

export function Minimap({
  sessions,
  range,
  onRange,
}: {
  sessions: Session[];
  range: TimeRange;
  onRange: (range: TimeRange) => void;
}) {
  const bars = Array.from({ length: 144 }, (_, i) => {
    const hour = i / 6;
    return sessions.reduce(
      (total, session) =>
        total +
        (hour >= session.start && hour <= session.end
          ? (session.events.length / 6) * (0.45 + ((i * 7) % 11) / 15)
          : 0),
      0,
    );
  });
  return (
    <div
      className="minimap"
      aria-label="履歴の全体表示"
      onPointerDown={(event) => {
        const bounds = event.currentTarget.getBoundingClientRect();
        const center = Math.min(
          24,
          Math.max(0, ((event.clientX - bounds.left) / bounds.width) * 24),
        );
        const span = range.end - range.start;
        const start = Math.max(0, Math.min(24 - span, center - span / 2));
        onRange({ start, end: start + span });
      }}
    >
      <svg viewBox="0 0 864 26" preserveAspectRatio="none" aria-hidden="true">
        {bars.map((value, i) => (
          <rect
            key={i}
            x={i * 6}
            y={25 - Math.min(value, 21)}
            width="4"
            height={Math.max(1, Math.min(value, 21))}
            rx="0.5"
          />
        ))}
      </svg>
      <span
        className="minimap-selection"
        style={{
          left: `${(range.start / 24) * 100}%`,
          width: `${((range.end - range.start) / 24) * 100}%`,
        }}
      >
        <i />
        <i />
      </span>
      <span className="minimap-label">00:00</span>
      <span className="minimap-label right">24:00</span>
    </div>
  );
}

export function Timeline({
  sessions,
  range,
  day,
  selected,
  selectedEvent,
  cost,
  directory,
  granularity,
  inViewOnly,
  onSelect,
}: TimelineProps) {
  const [collapsed, setCollapsed] = useState<string[]>([]);
  const axis = useRef<HTMLDivElement>(null);
  const [plotWidth, setPlotWidth] = useState(800);
  useEffect(() => {
    const observer = new ResizeObserver(([entry]) => setPlotWidth(entry.contentRect.width));
    if (axis.current) observer.observe(axis.current);
    return () => observer.disconnect();
  }, []);
  const span = range.end - range.start;
  const tickStep =
    [1 / 60, 1 / 12, 1 / 6, 0.25, 0.5, 1, 2, 3, 6, 12, 24].find(
      (step) => (step / span) * plotWidth >= 70,
    ) ?? 24;
  const firstTick = Math.ceil(range.start / tickStep) * tickStep;
  const ticks = Array.from(
    { length: Math.max(0, Math.floor((range.end - firstTick) / tickStep) + 1) },
    (_, index) => firstTick + index * tickStep,
  );
  const percent = (hour: number) => ((hour - range.start) / span) * 100;
  const visible = sessions.filter(
    (session) =>
      !inViewOnly ||
      (session.end + (session.day - day) * 24 >= range.start &&
        session.start + (session.day - day) * 24 <= range.end),
  );
  const groups = Array.from(new Set(visible.map((session) => session.project)));
  const plot = (session: Session, events: HistoryEvent[], agent = false) => (
    <div className={`timeline-plot ${selected === session.id ? 'selected-lane' : ''}`}>
      <div
        className="lane-span"
        style={{
          left: `${percent(session.start + (session.day - day) * 24)}%`,
          width: `${((session.end - session.start) / span) * 100}%`,
        }}
      />
      {agent && (
        <span
          className="agent-elbow"
          style={{ left: `${percent(session.start + (session.day - day) * 24 + 0.5)}%` }}
        />
      )}
      {cost
        ? Array.from({ length: 18 }, (_, i) => {
            const hour =
              session.start + (session.day - day) * 24 + ((session.end - session.start) * i) / 18;
            return (
              <button
                type="button"
                className="cost-bar"
                key={i}
                aria-label={`${session.title} 料金 $${(session.cost / 18).toFixed(3)}`}
                onClick={() => onSelect(session)}
                style={
                  {
                    left: `${percent(hour)}%`,
                    width: `${((session.end - session.start) / 18 / span) * 100}%`,
                    height: `${5 + ((i * 13) % 17)}px`,
                    '--source-color':
                      session.source === 'claude' ? 'var(--claude)' : 'var(--codex)',
                  } as CSSProperties
                }
              />
            );
          })
        : aggregate(events, granularity, span).map((events) => {
            const event = events[0];
            const end = events.at(-1)!;
            const startPosition = Math.max(-1, percent(event.start + (session.day - day) * 24));
            const rightPosition = Math.min(
              101,
              percent(end.start + end.duration + (session.day - day) * 24),
            );
            if (rightPosition < 0 || startPosition > 100) return null;
            return (
              <EventBlock
                key={event.id}
                kind={event.kind}
                label={events.length > 1 ? `${events.length} × ${event.name}` : event.name}
                failed={events.some((event) => event.status !== 'success')}
                selected={events.some((event) => event.id === selectedEvent)}
                onClick={() => onSelect(session, event)}
                style={{
                  left: `${startPosition}%`,
                  width: `max(3px, ${Math.max(0, rightPosition - startPosition)}%)`,
                }}
              />
            );
          })}
    </div>
  );

  return (
    <div
      className="timeline"
      aria-label={cost ? '料金のタイムライン' : 'イベントのタイムライン'}
      style={
        {
          '--tick-step': `${(tickStep / span) * 100}%`,
          '--grid-offset': `${((firstTick - range.start) / span) * plotWidth}px`,
        } as CSSProperties
      }
    >
      <div className="timeline-axis">
        <div className="axis-label">{directory ? 'ディレクトリ' : 'セッション / エージェント'}</div>
        <div className="axis-ticks" ref={axis}>
          <span className="timezone">
            {formatDate(day + Math.floor(range.start / 24))} · UTC+09:00
          </span>
          {ticks.map((hour) => (
            <span className="axis-tick" key={hour} style={{ left: `${percent(hour)}%` }}>
              {formatTime(hour)}
            </span>
          ))}
        </div>
      </div>
      <div className="timeline-scroll">
        {groups.map((project) => {
          const group = visible.filter((session) => session.project === project);
          const isCollapsed = collapsed.includes(project);
          return (
            <section className="timeline-group" key={project}>
              <div className="timeline-row group-row">
                <button
                  type="button"
                  className="lane-label"
                  aria-expanded={!isCollapsed}
                  onClick={() =>
                    setCollapsed(
                      isCollapsed
                        ? collapsed.filter((value) => value !== project)
                        : [...collapsed, project],
                    )
                  }
                >
                  <Icon name={isCollapsed ? 'ChevronRight' : 'ChevronDown'} size={12} />
                  <ProjectMark name={project} />
                  <strong>{project}</strong>
                  <span className="count">{group.length}</span>
                </button>
                <div className="timeline-plot group-plot">
                  {(isCollapsed || directory) &&
                    group.map((session) => (
                      <div key={session.id} className="directory-events">
                        {plot(session, session.events)}
                      </div>
                    ))}
                </div>
              </div>
              {!isCollapsed &&
                !directory &&
                group.map((session) => (
                  <div key={session.id}>
                    <div className="timeline-row">
                      <button
                        type="button"
                        className={`lane-label ${selected === session.id ? 'selected-lane' : ''}`}
                        onClick={() => onSelect(session)}
                        title={session.title}
                      >
                        <SourceLogo source={session.source} />
                        <span className="truncate">{session.title}</span>
                        {session.live && <span className="live-dot" />}
                        <span className="lane-count">{formatNumber(session.events.length)}</span>
                      </button>
                      {plot(session, session.events)}
                    </div>
                    {!cost &&
                      session.agents.map((agent, index) => (
                        <div className="timeline-row" key={agent}>
                          <button
                            type="button"
                            className="lane-label agent-label"
                            onClick={() =>
                              onSelect(
                                session,
                                session.events.find((event) => event.kind === 'agent'),
                              )
                            }
                          >
                            <span className="tree-line" />
                            <Icon name="Bot" size={12} />
                            <span className="truncate">{agent}</span>
                            <span className="lane-count">{6 + index * 3}</span>
                          </button>
                          {plot(session, session.events.slice(5 + index, 11 + index), true)}
                        </div>
                      ))}
                  </div>
                ))}
            </section>
          );
        })}
        <div className="timeline-fill">
          <div />
          <div />
        </div>
      </div>
      {day === 0 && range.start <= 17.2 && range.end >= 17.2 && (
        <div className="now-line-wrap">
          <div className="now-line" style={{ left: `${percent(17.2)}%` }}>
            <i />
          </div>
        </div>
      )}
    </div>
  );
}

export function CalendarSession({
  session,
  onClick,
  style,
}: {
  session: Session;
  onClick: () => void;
  style?: CSSProperties;
}) {
  return (
    <button
      type="button"
      className="calendar-session"
      aria-label={session.title}
      onClick={onClick}
      style={{ '--project-color': projectColor(session.project), ...style } as CSSProperties}
    >
      <span className="calendar-session-heading">
        <SourceLogo source={session.source} size={11} />
        <strong>{session.project}</strong>
        {session.live && <span className="live-dot" />}
      </span>
      <span className="truncate">{session.title}</span>
      <small>
        {formatTime(session.start)}–{formatTime(session.end)}
      </small>
    </button>
  );
}
