import type { CSSProperties } from 'react';
import { formatDuration, formatTime, projectColor, type Session, type View } from '../data';
import { CalendarSession } from './Timeline';

const weekdays = ['月', '火', '水', '木', '金', '土', '日'];

function dateLabel(day: number) {
  const date = new Date(Date.UTC(2026, 9, 26 + day));
  return { day: date.getUTCDate(), month: date.getUTCMonth() + 1 };
}

export function CalendarView({
  mode,
  sessions,
  offset,
  selectedDay,
  onSelect,
  onDay,
}: {
  mode: Extract<View, 'day' | 'week' | 'month'>;
  sessions: Session[];
  offset: number;
  selectedDay: number;
  onSelect: (session: Session) => void;
  onDay: (day: number) => void;
}) {
  if (mode === 'month')
    return <MonthView sessions={sessions} offset={offset} onDay={onDay} onSelect={onSelect} />;
  const days =
    mode === 'week' ? Array.from({ length: 7 }, (_, i) => i + offset * 7) : [selectedDay + offset];
  const pixelsPerHour = mode === 'day' ? 100 : 76;
  const startHour = 8;
  const height = 12 * pixelsPerHour;
  return (
    <div
      className={`calendar-view ${mode}`}
      aria-label={mode === 'week' ? '週カレンダー' : '日カレンダー'}
    >
      <div className="calendar-day-head">
        <div className="calendar-time-head">JST</div>
        {days.map((day) => {
          const items = sessions.filter((session) => session.day === day);
          return (
            <button
              type="button"
              className={day === 0 ? 'today' : ''}
              key={day}
              onClick={() => onDay(day)}
            >
              <span>
                {weekdays[((day % 7) + 7) % 7]} <b>{dateLabel(day).day}</b>
              </span>
              <small>
                {items.length
                  ? `${formatDuration(items.reduce((sum, session) => sum + session.end - session.start, 0))} · ${items.length} 件`
                  : '—'}
              </small>
            </button>
          );
        })}
      </div>
      <div className="calendar-scroll">
        <div className="calendar-hours" style={{ height }}>
          <div className="calendar-time-labels">
            {Array.from({ length: 13 }, (_, i) => (
              <span key={i} style={{ top: i * pixelsPerHour }}>
                {String(i + startHour).padStart(2, '0')}:00
              </span>
            ))}
          </div>
          {days.map((day) => {
            const items = sessions
              .filter((session) => session.day === day)
              .sort((a, b) => a.start - b.start);
            // 重なるセッションを別列に並べる。固定サンプルでも日・週で同じ衝突判定を使う。
            const columnEnds: number[] = [];
            const positions = items.map((session) => {
              let column = columnEnds.findIndex((end) => end <= session.start);
              if (column < 0) column = columnEnds.length;
              columnEnds[column] = session.end;
              return { session, column };
            });
            const columns = Math.max(1, columnEnds.length);
            return (
              <div
                className={`calendar-day-column ${day === 0 ? 'today' : ''}`}
                key={day}
                style={{ backgroundSize: `100% ${pixelsPerHour}px` }}
              >
                {positions.map(({ session, column }) => (
                  <CalendarSession
                    key={session.id}
                    session={session}
                    onClick={() => onSelect(session)}
                    style={{
                      top: (session.start - startHour) * pixelsPerHour,
                      height: Math.max(26, (session.end - session.start) * pixelsPerHour - 3),
                      left: `calc(${(column / columns) * 100}% + 2px)`,
                      width: `calc(${100 / columns}% - 4px)`,
                    }}
                  />
                ))}
                {day === 0 && (
                  <span
                    className="calendar-now"
                    style={{ top: (17.2 - startHour) * pixelsPerHour }}
                  >
                    <i />
                  </span>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function MonthView({
  sessions,
  offset,
  onDay,
  onSelect,
}: {
  sessions: Session[];
  offset: number;
  onDay: (day: number) => void;
  onSelect: (session: Session) => void;
}) {
  const first = new Date(Date.UTC(2026, 9 + offset, 1));
  const mondayOffset = (first.getUTCDay() + 6) % 7;
  const firstDay =
    Math.round((first.getTime() - Date.UTC(2026, 9, 26)) / 86_400_000) - mondayOffset;
  const activeOf = (day: number) =>
    sessions
      .filter((session) => session.day === day)
      .reduce((sum, session) => sum + session.end - session.start, 0);
  const maxActive = Math.max(1e-9, ...Array.from({ length: 42 }, (_, i) => activeOf(firstDay + i)));
  return (
    <div className="month-view" aria-label="月カレンダー">
      <div className="month-weekdays">
        {weekdays.map((day) => (
          <span key={day}>{day}</span>
        ))}
      </div>
      <div className="month-grid">
        {Array.from({ length: 42 }, (_, index) => {
          const day = firstDay + index;
          const date = dateLabel(day);
          const items = sessions.filter((session) => session.day === day);
          // 稼働時間は上端の帯の段階（heat の 5 段）で示す。本体と同じく平方根で段を割り当てる
          const heat = Math.round(Math.sqrt(activeOf(day) / maxActive) * 4) + 1;
          return (
            <div
              key={day}
              className={`month-cell ${date.month !== first.getUTCMonth() + 1 ? 'outside' : ''} ${items.length ? 'has-sessions' : ''}`}
              style={
                items.length ? ({ '--heat': `var(--heat-${heat})` } as CSSProperties) : undefined
              }
            >
              <button
                type="button"
                className="month-day"
                aria-label={`${date.month}月${date.day}日を開く`}
                onClick={() => onDay(day)}
              >
                <b className={day === 0 ? 'today' : ''}>
                  {date.day === 1 ? `${date.month}/1` : date.day}
                </b>
                {items.length > 0 && (
                  <small>
                    {formatDuration(
                      items.reduce((sum, session) => sum + session.end - session.start, 0),
                    )}
                  </small>
                )}
              </button>
              <div className="month-sessions">
                {items.slice(0, 4).map((session) => (
                  <button
                    type="button"
                    className="month-session"
                    key={session.id}
                    onClick={() => onSelect(session)}
                    title={session.title}
                  >
                    <i style={{ background: projectColor(session.project) }} />
                    <small>{formatTime(session.start)}</small>
                    <span className="truncate">{session.title}</span>
                  </button>
                ))}
                {items.length > 4 && (
                  <button type="button" className="month-more" onClick={() => onDay(day)}>
                    ほか {items.length - 4} 件
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
