import { useState, type CSSProperties, type ReactNode } from 'react';
import {
  formatNumber,
  formatTokens,
  projectColor,
  projects,
  type HistoryEvent,
  type Session,
} from '../data';
import {
  Button,
  EmptyState,
  Icon,
  ProjectMark,
  Segmented,
  SourceLogo,
  StatusText,
} from './primitives';

type Metric = 'tokens' | 'cost' | 'turns' | 'sessions' | 'tools' | 'failures';
const metrics: Array<{ key: Metric; label: string; hint?: string }> = [
  { key: 'tokens', label: 'トークン' },
  { key: 'cost', label: '料金', hint: 'ログの使用量と料金表による API 換算の推定値（USD）' },
  { key: 'turns', label: 'ターン' },
  { key: 'sessions', label: 'セッション' },
  { key: 'tools', label: 'ツール呼び出し' },
  { key: 'failures', label: '失敗・拒否・中断' },
];
const isTool = (event: HistoryEvent) => !['user', 'assistant', 'thinking'].includes(event.kind);
function amount(session: Session, metric: Metric): number {
  switch (metric) {
    case 'tokens':
      return session.tokens;
    case 'cost':
      return session.cost;
    case 'turns':
      return session.events.filter((event) => event.kind === 'user').length;
    case 'sessions':
      return 1;
    case 'tools':
      return session.events.filter(isTool).length;
    case 'failures':
      return session.events.filter((event) => event.status !== 'success').length;
  }
}
const total = (sessions: Session[], metric: Metric) =>
  sessions.reduce((sum, session) => sum + amount(session, metric), 0);
const format = (value: number, metric: Metric) =>
  metric === 'tokens'
    ? value
      ? formatTokens(value)
      : '0'
    : metric === 'cost'
      ? `$${value.toFixed(2)}`
      : formatNumber(Math.round(value));

function Section({
  title,
  aside,
  children,
}: {
  title: string;
  aside?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="stats-section">
      <div className="stats-section-heading">
        <h2>{title}</h2>
        {aside}
      </div>
      {children}
    </section>
  );
}

export function Statistics({
  sessions,
  project,
  loading,
  onProject,
  onSession,
}: {
  sessions: Session[];
  project: string | null;
  loading?: boolean;
  onProject: (project: string | null) => void;
  onSession: (session: Session) => void;
}) {
  const [period, setPeriod] = useState('week');
  const [metric, setMetric] = useState<Metric>('tokens');
  const days = period === 'week' ? 7 : period === 'month' ? 30 : period === 'quarter' ? 90 : 3;
  const activeDays = Math.max(1, new Set(sessions.map((session) => session.day)).size);
  const value = total(sessions, metric);
  const bySource = [
    sessions.filter((session) => session.source === 'claude'),
    sessions.filter((session) => session.source === 'codex'),
  ];
  const metricLabel = metrics.find((item) => item.key === metric)!.label;
  const ranking = projects
    .map((project) => ({
      ...project,
      sessions: sessions.filter((session) => session.project === project.name),
    }))
    .filter((project) => project.sessions.length)
    .sort((a, b) => total(b.sessions, metric) - total(a.sessions, metric));
  const rankedSessions = [...sessions].sort((a, b) => amount(b, metric) - amount(a, metric));
  const chartDays = Math.min(days, 30);
  const chartValues = Array.from({ length: chartDays }, (_, i) =>
    bySource.map((source) =>
      total(
        source.filter((session) => session.day === i),
        metric,
      ),
    ),
  );
  const max = Math.max(1, ...chartValues.map((values) => values.reduce((a, b) => a + b, 0)));
  const tools = Array.from(
    new Set(
      sessions.flatMap((session) => session.events.filter(isTool).map((event) => event.name)),
    ),
  )
    .map((name) => ({
      name,
      events: sessions.flatMap((session) => session.events.filter((event) => event.name === name)),
    }))
    .sort((a, b) => b.events.length - a.events.length);
  const hourly = ['月', '火', '水', '木', '金', '土', '日'].map((day, index) => ({
    day,
    counts: Array.from({ length: 24 }, (_, hour) =>
      sessions
        .filter((session) => session.day === index)
        .reduce(
          (sum, session) =>
            sum + session.events.filter((event) => Math.floor(event.start) === hour).length,
          0,
        ),
    ),
  }));
  const maxCount = Math.max(1, ...hourly.flatMap((row) => row.counts));
  return (
    <div className="statistics-view" aria-label="統計">
      <header className="statistics-header">
        <strong>統計</strong>
        {loading && <StatusText>集計中…</StatusText>}
        {project && (
          <Button className="focus-chip" onClick={() => onProject(null)}>
            {project}
            <Icon name="X" size={11} />
          </Button>
        )}
        <span
          className="statistics-subtitle"
          title="集計はローカル時間で行い、サブエージェントは起動元のセッションに含みます。"
        >
          2026/10/26–
          {period === 'week'
            ? '11/1'
            : period === 'month'
              ? '11/24'
              : period === 'quarter'
                ? '2027/1/23'
                : '10/28'}{' '}
          · 稼働 {new Set(sessions.map((session) => session.day)).size} / {days} 日
        </span>
        <Segmented
          label="統計の期間"
          options={[
            { value: 'week', label: '7日間' },
            { value: 'month', label: '30日間' },
            { value: 'quarter', label: '90日間' },
            { value: 'all', label: '全期間' },
          ]}
          value={period}
          onChange={setPeriod}
        />
      </header>
      <div className={`statistics-scroll ${loading ? 'stale' : ''}`} aria-busy={loading}>
        <div className="metric-row">
          {metrics.map((item) => {
            const metricTotal = total(sessions, item.key);
            const calls = total(sessions, 'tools');
            return (
              <button
                type="button"
                className={`metric ${metric === item.key ? 'selected' : ''}`}
                key={item.key}
                aria-pressed={metric === item.key}
                title={item.hint}
                onClick={() => setMetric(item.key)}
              >
                <span>{item.label}</span>
                <strong>{format(metricTotal, item.key)}</strong>
                <small>
                  {item.key === 'failures'
                    ? `呼び出しの ${calls ? Math.round((metricTotal / calls) * 100) : 0}%`
                    : `稼働日あたり ${format(metricTotal / activeDays, item.key)}`}
                </small>
              </button>
            );
          })}
        </div>
        {!sessions.length ? (
          <EmptyState filtered />
        ) : (
          <>
            <Section
              title={`${metricLabel}の推移`}
              aside={
                <div className="chart-legend">
                  <span>
                    <i className="claude-color" />
                    Claude Code
                  </span>
                  <span>
                    <i className="codex-color" />
                    Codex
                  </span>
                </div>
              }
            >
              <div className="trend-chart">
                <div className="chart-y-labels">
                  {[1, 0.75, 0.5, 0.25, 0].map((value) => (
                    <span key={value}>{format(max * value, metric)}</span>
                  ))}
                </div>
                <div className="chart-columns">
                  {chartValues.map((values, index) => (
                    <div
                      className="chart-column"
                      key={index}
                      title={`10/${26 + index} · ${format(values[0] + values[1], metric)}`}
                    >
                      <div className="chart-stack">
                        {values.map((value, source) => (
                          <span
                            key={source}
                            className={source === 0 ? 'claude-color' : 'codex-color'}
                            style={{ height: `${(value / max) * 100}%` }}
                          />
                        ))}
                      </div>
                      <small>
                        {chartDays <= 10 || index % 5 === 0
                          ? `${index < 6 ? 10 : 11}/${index < 6 ? 26 + index : index - 5}`
                          : ''}
                      </small>
                    </div>
                  ))}
                </div>
              </div>
            </Section>
            <div className="stats-pair">
              <Section title="プロジェクト">
                {ranking.map((item, index) => (
                  <button
                    type="button"
                    className="ranking-row"
                    key={item.name}
                    onClick={() => onProject(project === item.name ? null : item.name)}
                  >
                    <span className="rank">{index + 1}</span>
                    <ProjectMark name={item.name} />
                    <span className="ranking-label">
                      {item.name}
                      <i
                        style={{
                          width: `${(total(item.sessions, metric) / Math.max(1, total(ranking[0].sessions, metric))) * 100}%`,
                          background: projectColor(item.name),
                        }}
                      />
                    </span>
                    <strong>{format(total(item.sessions, metric), metric)}</strong>
                  </button>
                ))}
              </Section>
              <Section title="セッション">
                {rankedSessions.slice(0, 6).map((session, index) => (
                  <button
                    type="button"
                    className="ranking-row session-ranking"
                    key={session.id}
                    onClick={() => onSession(session)}
                  >
                    <span className="rank">{index + 1}</span>
                    <SourceLogo source={session.source} />
                    <span className="ranking-label">
                      <span className="truncate">{session.title}</span>
                      <small>
                        {session.project} · 10/{26 + session.day}
                      </small>
                    </span>
                    <strong>{format(amount(session, metric), metric)}</strong>
                  </button>
                ))}
              </Section>
            </div>
            <div className="stats-pair">
              <Section title="曜日と時間帯">
                <div className="hours-legend">
                  <span>0</span>
                  <span>6</span>
                  <span>12</span>
                  <span>18</span>
                  <span>24時</span>
                </div>
                <div className="heatmap">
                  {hourly.map(({ day, counts }) => (
                    <div key={day}>
                      <span>{day}</span>
                      {counts.map((count, hour) => (
                        <i
                          key={hour}
                          title={`${day}曜 ${hour}時 · ${count} 件`}
                          style={{
                            background: count
                              ? `var(--heat-${Math.max(1, Math.ceil((count / maxCount) * 5))})`
                              : undefined,
                          }}
                        />
                      ))}
                    </div>
                  ))}
                </div>
                <div className="heatmap-scale">
                  <span>少</span>
                  {[1, 2, 3, 4, 5].map((level) => (
                    <i key={level} style={{ background: `var(--heat-${level})` }} />
                  ))}
                  <span>多</span>
                </div>
              </Section>
              <Section title="モデル">
                <div className="model-breakdown">
                  <div
                    className="model-donut"
                    style={
                      {
                        '--split': `${value ? (total(bySource[0], metric) / value) * 100 : 0}%`,
                      } as CSSProperties
                    }
                  >
                    <div />
                  </div>
                  <div className="model-legend">
                    {bySource.map((items, index) => (
                      <div key={index}>
                        <i className={index === 0 ? 'claude-color' : 'codex-color'} />
                        <span>
                          {index === 0 ? 'claude-sonnet-4-5' : 'gpt-5-codex'}
                          <small>{format(total(items, metric), metric)}</small>
                        </span>
                        <strong>
                          {value ? Math.round((total(items, metric) / value) * 100) : 0}%
                        </strong>
                      </div>
                    ))}
                  </div>
                </div>
              </Section>
            </div>
            <Section title="ツール">
              <div className="table-scroll">
                <table className="tools-table">
                  <thead>
                    <tr>
                      <th>ツール</th>
                      <th>呼び出し</th>
                      <th>失敗</th>
                      <th>拒否 / 中断</th>
                      <th>中央値</th>
                      <th>P95</th>
                      <th>所要時間</th>
                    </tr>
                  </thead>
                  <tbody>
                    {tools.map((tool) => {
                      const durations = tool.events
                        .map((event) => event.duration * 3600)
                        .sort((a, b) => a - b);
                      const median = durations[Math.floor(durations.length / 2)];
                      const p95 =
                        durations[
                          Math.min(durations.length - 1, Math.floor(durations.length * 0.95))
                        ];
                      const failed = tool.events.filter((event) => event.status === 'failed');
                      return (
                        <tr key={tool.name}>
                          <td>{tool.name}</td>
                          <td>{tool.events.length}</td>
                          <td className={failed.length ? 'failed' : ''}>{failed.length}</td>
                          <td>
                            {
                              tool.events.filter(
                                (event) =>
                                  event.status === 'rejected' || event.status === 'interrupted',
                              ).length
                            }
                          </td>
                          <td>{median.toFixed(1)}秒</td>
                          <td>{p95.toFixed(1)}秒</td>
                          <td>
                            <div className="duration-track">
                              <i
                                style={{
                                  width: `${Math.min(100, (Math.log10(p95 + 1) / 4) * 100)}%`,
                                }}
                              />
                            </div>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </Section>
            <Section title="読み込み元">
              <div className="source-totals">
                {bySource.map((items, index) => (
                  <div key={index}>
                    <SourceLogo source={index === 0 ? 'claude' : 'codex'} size={20} />
                    <span>{index === 0 ? 'Claude Code' : 'Codex'}</span>
                    <strong>{format(total(items, metric), metric)}</strong>
                    <small>{items.length} セッション</small>
                  </div>
                ))}
              </div>
            </Section>
          </>
        )}
      </div>
    </div>
  );
}
