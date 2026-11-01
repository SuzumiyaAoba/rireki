import { useState, type CSSProperties, type ReactNode } from 'react';
import {
  formatNumber,
  formatTokens,
  projectColor,
  projects,
  type HistoryEvent,
  type Session,
  type Source,
} from '../data';
import { Button, EmptyState, Icon, ProjectMark, Segmented, SourceLogo } from './primitives';

type Metric = 'tokens' | 'cost' | 'turns' | 'sessions' | 'tools' | 'failures';
const metrics: Array<{ key: Metric; label: string }> = [
  { key: 'tokens', label: 'トークン' },
  { key: 'cost', label: '推定料金' },
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

function Card({
  title,
  subtitle,
  children,
  className = '',
}: {
  title: string;
  subtitle?: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={`stats-card ${className}`}>
      <div className="stats-card-heading">
        <h2>{title}</h2>
        {subtitle && <span>{subtitle}</span>}
      </div>
      {children}
    </section>
  );
}

export function Statistics({
  sessions,
  source,
  project,
  loading,
  onProject,
  onSession,
}: {
  sessions: Session[];
  source: Source;
  project: string | null;
  loading?: boolean;
  onProject: (project: string | null) => void;
  onSession: (session: Session) => void;
}) {
  const [period, setPeriod] = useState('week');
  const [metric, setMetric] = useState<Metric>('tokens');
  const days = period === 'week' ? 7 : period === 'month' ? 30 : period === 'quarter' ? 90 : 3;
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
  return (
    <div className="statistics-view" aria-label="使用量と実行結果の統計">
      <header className="statistics-header">
        <Icon name="ChartColumn" />
        <strong>統計</strong>
        {project && (
          <Button icon="Folder" className="focus-chip" onClick={() => onProject(null)}>
            {project}
            <Icon name="X" size={11} />
          </Button>
        )}
        <span className="statistics-subtitle">
          2026/10/26–
          {period === 'week'
            ? '11/1'
            : period === 'month'
              ? '11/24'
              : period === 'quarter'
                ? '2027/1/23'
                : '10/28'}{' '}
          · 稼働 {new Set(sessions.map((session) => session.day)).size} / {days} 日 ·{' '}
          {source === 'all'
            ? 'Claude Code と Codex'
            : source === 'claude'
              ? 'Claude Code'
              : 'Codex'}
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
      {loading ? (
        <EmptyState loading />
      ) : (
        <div className="statistics-scroll">
          <div className="metric-tiles">
            {metrics.map((item) => {
              const metricTotal = total(sessions, item.key);
              const claude = total(bySource[0], item.key);
              return (
                <button
                  type="button"
                  className={`metric-tile ${metric === item.key ? 'selected' : ''}`}
                  key={item.key}
                  aria-pressed={metric === item.key}
                  onClick={() => setMetric(item.key)}
                >
                  <span>{item.label}</span>
                  <strong>{format(metricTotal, item.key)}</strong>
                  <small>
                    稼働日あたり{' '}
                    {format(
                      metricTotal /
                        Math.max(1, new Set(sessions.map((session) => session.day)).size),
                      item.key,
                    )}
                  </small>
                  {source === 'all' && (
                    <span className="source-split">
                      <i style={{ width: `${metricTotal ? (claude / metricTotal) * 100 : 0}%` }} />
                      <i />
                    </span>
                  )}
                </button>
              );
            })}
          </div>
          {!sessions.length ? (
            <Card title="この期間の履歴はありません">
              <EmptyState filtered />
            </Card>
          ) : (
            <>
              <Card title={`${metricLabel}の推移`} subtitle={`合計 ${format(value, metric)}`}>
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
              </Card>
              <div className="stats-pair">
                <Card title="プロジェクト" subtitle={metricLabel}>
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
                      <Icon name="ChevronRight" size={12} />
                    </button>
                  ))}
                </Card>
                <Card title="セッション" subtitle={metricLabel}>
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
                      <Icon name="ChevronRight" size={12} />
                    </button>
                  ))}
                </Card>
              </div>
              <div className="stats-pair">
                <Card title="曜日と時間帯" subtitle="イベント数">
                  <div className="hours-legend">
                    <span>0</span>
                    <span>6</span>
                    <span>12</span>
                    <span>18</span>
                    <span>24時</span>
                  </div>
                  <div className="heatmap">
                    {['月', '火', '水', '木', '金', '土', '日'].map((day, index) => (
                      <div key={day}>
                        <span>{day}</span>
                        {Array.from({ length: 24 }, (_, hour) => {
                          const count = sessions
                            .filter((session) => session.day === index)
                            .reduce(
                              (sum, session) =>
                                sum +
                                session.events.filter((event) => Math.floor(event.start) === hour)
                                  .length,
                              0,
                            );
                          return (
                            <i
                              key={hour}
                              title={`${day}曜 ${hour}時 · ${count} 件`}
                              style={{
                                background: count
                                  ? `color-mix(in srgb, #3987e5 ${Math.min(100, 20 + count * 4)}%, var(--panel))`
                                  : undefined,
                              }}
                            />
                          );
                        })}
                      </div>
                    ))}
                  </div>
                  <div className="heatmap-scale">
                    <span>少</span>
                    {['#184f95', '#256abf', '#3987e5', '#6da7ec', '#9ec5f4'].map((color) => (
                      <i key={color} style={{ background: color }} />
                    ))}
                    <span>多</span>
                  </div>
                </Card>
                <Card title="モデル" subtitle={metricLabel}>
                  <div className="model-breakdown">
                    <div
                      className="model-donut"
                      style={
                        {
                          '--split': `${value ? (total(bySource[0], metric) / value) * 100 : 0}%`,
                        } as CSSProperties
                      }
                    >
                      <div>
                        <strong>{format(value, metric)}</strong>
                        <small>合計</small>
                      </div>
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
                </Card>
              </div>
              <Card title="ツール" subtitle="呼び出し・失敗・所要時間">
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
                        return (
                          <tr key={tool.name}>
                            <td>
                              <Icon name="Wrench" size={12} />
                              {tool.name}
                            </td>
                            <td>{tool.events.length}</td>
                            <td
                              className={
                                tool.events.some((event) => event.status === 'failed')
                                  ? 'removed'
                                  : ''
                              }
                            >
                              {tool.events.filter((event) => event.status === 'failed').length}
                            </td>
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
              </Card>
              <Card title="読み込み元" subtitle={metricLabel}>
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
              </Card>
            </>
          )}
          <p className="statistics-note">
            料金はログの使用量と料金表による API
            換算の推定値（USD）です。サブエージェントは起動元のセッションに含めます。
          </p>
        </div>
      )}
    </div>
  );
}
