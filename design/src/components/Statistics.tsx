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
import { eventKinds, type EventKind } from '../tokens';
import {
  Button,
  EmptyState,
  eventIcon,
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

type ToolRow = {
  name: string;
  kind: EventKind;
  calls: number;
  claude: number;
  codex: number;
  failed: number;
  rejected: number;
  median: number;
  p95: number;
};

const TOOL_LIMIT = 10;
// 所要時間の対数軸の目盛り（秒）と、失敗の帯の上限（呼び出しに対する割合）。本体 statistics/tools.rs と同じ
const LOG_TICKS = [0.001, 0.01, 0.1, 1, 10, 60, 600, 3600, 36000];
const FAIL_SCALES = [0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1];

const tickLabel = (seconds: number) =>
  seconds < 1
    ? `${Math.round(seconds * 1000)}ms`
    : seconds < 60
      ? `${seconds}s`
      : seconds < 3600
        ? `${seconds / 60}m`
        : `${seconds / 3600}h`;
const formatSeconds = (seconds: number) =>
  seconds < 1
    ? `${Math.round(seconds * 1000)}ms`
    : seconds < 60
      ? `${seconds.toFixed(1)}s`
      : seconds < 3600
        ? `${(seconds / 60).toFixed(1)}m`
        : `${(seconds / 3600).toFixed(1)}h`;

// ツールごとの 1 行: 印・名前・呼び出しの積み棒・件数・失敗の帯・失敗率・所要時間の対数軸・中央値・P95。
// 列は幅に応じて右から省く（本体と同じく、軸 → 失敗の帯 → P95 の順）。
function ToolsTable({ tools: rows, metric }: { tools: ToolRow[]; metric: Metric }) {
  if (!rows.length) return <EmptyState message="この期間にツール呼び出しはありません" />;
  // 指標が失敗・拒否・中断のときは失敗の多い順（同数は呼び出しの順）
  const tools =
    metric === 'failures'
      ? [...rows].sort(
          (a, b) => b.failed + b.rejected - (a.failed + a.rejected) || b.calls - a.calls,
        )
      : rows;
  const shown = tools.slice(0, TOOL_LIMIT);
  const rest = tools.length - shown.length;
  const maxCalls = Math.max(...shown.map((tool) => tool.calls));
  const maxRate = Math.max(...shown.map((tool) => (tool.failed + tool.rejected) / tool.calls));
  const scale = FAIL_SCALES.find((value) => value >= maxRate) ?? 1;
  // 軸は表示中のツールの中央値の最小と P95 の最大を目盛りに丸めた範囲で、少なくとも 1 桁の幅を持つ
  const low =
    [...LOG_TICKS]
      .slice(0, -1)
      .reverse()
      .find((tick) => tick <= Math.min(...shown.map((t) => t.median))) ?? LOG_TICKS[0];
  const high =
    LOG_TICKS.find((tick) => tick >= Math.max(low * 10, ...shown.map((t) => t.p95))) ??
    LOG_TICKS[LOG_TICKS.length - 1];
  const place = (seconds: number) =>
    ((Math.log10(Math.min(Math.max(seconds, low), high)) - Math.log10(low)) /
      (Math.log10(high) - Math.log10(low))) *
    100;
  const ticks = LOG_TICKS.filter((tick) => tick >= low && tick <= high);
  return (
    <div className="tools-grid" role="table" aria-label="ツール別の集計">
      <div className="tool-row tool-head" role="row">
        <span />
        <span />
        <span />
        <span role="columnheader">呼び出し</span>
        <span className="tool-fail">
          <small>0</small>
          <small>{Math.round(scale * 100)}%</small>
        </span>
        <span role="columnheader">失敗率</span>
        <span className="tool-axis">
          {ticks.map((tick, index) => (
            <small
              key={tick}
              className={index === 0 ? 'first' : index === ticks.length - 1 ? 'last' : ''}
              style={{ left: `${place(tick)}%` }}
            >
              {tickLabel(tick)}
            </small>
          ))}
        </span>
        <span role="columnheader">中央値</span>
        <span className="tool-p95" role="columnheader">
          P95
        </span>
      </div>
      {shown.map((tool) => {
        const failures = tool.failed + tool.rejected;
        const share = (value: number) => `${(value / tool.calls) * 100}%`;
        return (
          <div className="tool-row" role="row" key={tool.name}>
            <span className="tool-mark" style={{ color: eventKinds[tool.kind].color }}>
              <Icon name={eventIcon(tool.name, tool.kind)} size={12} />
            </span>
            <span className="tool-name" title={tool.name}>
              {tool.name}
            </span>
            <span className="tool-bar">
              <span style={{ width: `${(tool.calls / maxCalls) * 100}%` }}>
                <i className="claude-color" style={{ width: share(tool.claude) }} />
                <i className="codex-color" style={{ width: share(tool.codex) }} />
              </span>
            </span>
            <span className="tool-num" role="cell">
              {tool.calls}
            </span>
            <span className="tool-fail" aria-hidden>
              <i
                className="err-color"
                style={{ width: `${(tool.failed / tool.calls / scale) * 100}%` }}
              />
              <i
                className="warn-color"
                style={{ width: `${(tool.rejected / tool.calls / scale) * 100}%` }}
              />
            </span>
            <span className={`tool-num ${failures ? '' : 'none'}`} role="cell">
              {failures ? `${((failures / tool.calls) * 100).toFixed(1)}%` : '—'}
            </span>
            <span className="tool-axis" aria-hidden>
              {ticks.map((tick) => (
                <small key={tick} style={{ left: `${place(tick)}%` }} />
              ))}
              <b
                style={{
                  left: `${place(tool.median)}%`,
                  width: `${place(tool.p95) - place(tool.median)}%`,
                }}
              />
              <i className="median" style={{ left: `${place(tool.median)}%` }} />
              <i className="p95" style={{ left: `${place(tool.p95)}%` }} />
            </span>
            <span className="tool-num" role="cell">
              {formatSeconds(tool.median)}
            </span>
            <span className="tool-num tool-p95 none" role="cell">
              {formatSeconds(tool.p95)}
            </span>
          </div>
        );
      })}
      {rest > 0 && (
        <div className="tool-row tool-other" role="row">
          <span />
          <span className="tool-name">その他 {rest} 種類</span>
          <span />
          <span className="tool-num" role="cell">
            {tools.slice(TOOL_LIMIT).reduce((sum, tool) => sum + tool.calls, 0)}
          </span>
        </div>
      )}
      <div className="chart-legend">
        <span>
          <i className="claude-color" />
          Claude
        </span>
        <span>
          <i className="codex-color" />
          Codex
        </span>
        <span>
          <i className="err-color" />
          失敗
        </span>
        <span>
          <i className="warn-color" />
          拒否・中断
        </span>
        <span className="axis-legend">
          <i className="median" />
          中央値
        </span>
        <span className="axis-legend">
          <i className="p95" />
          P95
        </span>
      </div>
      <p className="stats-note">
        失敗率は呼び出しに対する割合。所要時間は結果のある呼び出しの区間分布から求めた近似値（誤差
        6.25% 以内）で、サブエージェントは配下の処理を含みます
      </p>
    </div>
  );
}

type Hourly = Array<{ day: string; counts: number[] }>;

// 24 時間を円周（上が 0 時、時計回り）、曜日を内側から月〜日の同心の輪にした時間帯のグラフ。
// 寸法は本体 src/app/statistics/radial.rs に合わせる（外側の余白 24、輪の隙間 2、内側の半径は外径の 0.32 倍）。
function RadialHours({ hourly, maxCount }: { hourly: Hourly; maxCount: number }) {
  const size = 360;
  const center = size / 2;
  const outer = center - 24;
  const inner = outer * 0.32;
  const ringGap = 2;
  const angleGap = 0.028;
  const pitch = (outer - inner) / hourly.length;
  const thickness = pitch - ringGap;
  const point = (radius: number, angle: number) =>
    `${(center + radius * Math.sin(angle)).toFixed(2)} ${(center - radius * Math.cos(angle)).toFixed(2)}`;
  const sector = (r0: number, r1: number, a0: number, a1: number) =>
    `M${point(r1, a0)}A${r1} ${r1} 0 0 1 ${point(r1, a1)}L${point(r0, a1)}A${r0} ${r0} 0 0 0 ${point(r0, a0)}Z`;
  const total = hourly.reduce((sum, row) => sum + row.counts.reduce((a, b) => a + b, 0), 0);
  return (
    <svg
      className="radial-hours"
      viewBox={`0 0 ${size} ${size}`}
      role="img"
      aria-label="時間帯の円形グラフ"
    >
      {hourly.map(({ day, counts }, ring) => {
        const r0 = inner + ring * pitch;
        return counts.map((count, hour) => (
          <path
            key={`${day}${hour}`}
            d={sector(
              r0,
              r0 + thickness,
              (hour / 24) * Math.PI * 2 + angleGap / 2,
              ((hour + 1) / 24) * Math.PI * 2 - angleGap / 2,
            )}
            fill={
              count
                ? `var(--heat-${Math.max(1, Math.ceil((count / maxCount) * 5))})`
                : 'var(--raised)'
            }
          >
            <title>{`${day}曜 ${hour}時 · ${count} 件`}</title>
          </path>
        ));
      })}
      {[0, 3, 6, 9, 12, 15, 18, 21].map((hour) => {
        const angle = (hour / 24) * Math.PI * 2;
        return (
          <text
            key={hour}
            x={center + (outer + 13) * Math.sin(angle)}
            y={center - (outer + 13) * Math.cos(angle)}
            textAnchor="middle"
            dominantBaseline="central"
          >
            {hour}
          </text>
        );
      })}
      <circle cx={center} cy={center} r={inner} fill="var(--backdrop)" />
      <text className="radial-label" x={center} y={center - 8} textAnchor="middle">
        期間合計
      </text>
      <text className="radial-total" x={center} y={center + 12} textAnchor="middle">
        {total}
      </text>
    </svg>
  );
}

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
  const [hoursView, setHoursView] = useState<'radial' | 'table'>('radial');
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
  const tools: ToolRow[] = Array.from(
    new Set(
      sessions.flatMap((session) => session.events.filter(isTool).map((event) => event.name)),
    ),
  )
    .map((name) => {
      const events = sessions.flatMap((session) =>
        session.events.filter((event) => event.name === name),
      );
      const durations = events.map((event) => event.duration * 3600).sort((a, b) => a - b);
      const calls = (source: Source) =>
        sessions
          .filter((session) => session.source === source)
          .reduce((sum, session) => sum + session.events.filter((e) => e.name === name).length, 0);
      return {
        name,
        kind: events[0].kind,
        calls: events.length,
        claude: calls('claude'),
        codex: calls('codex'),
        failed: events.filter((event) => event.status === 'failed').length,
        rejected: events.filter(
          (event) => event.status === 'rejected' || event.status === 'interrupted',
        ).length,
        median: durations[Math.floor(durations.length / 2)],
        p95: durations[Math.min(durations.length - 1, Math.floor(durations.length * 0.95))],
      };
    })
    .sort((a, b) => b.calls - a.calls || a.name.localeCompare(b.name));
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
          <EmptyState message="この期間の履歴はありません" />
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
              <Section
                title="時間帯"
                aside={
                  <Segmented
                    label="時間帯の表示"
                    options={[
                      { value: 'radial', label: '円形' },
                      { value: 'table', label: '表' },
                    ]}
                    value={hoursView}
                    onChange={setHoursView}
                  />
                }
              >
                {hoursView === 'radial' ? (
                  <>
                    <RadialHours hourly={hourly} maxCount={maxCount} />
                    <p className="radial-caption">内側から月〜日 · 上が0時、時計回りに24時間</p>
                  </>
                ) : (
                  <>
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
                  </>
                )}
                <div className="heatmap-scale">
                  <span>少</span>
                  {[1, 2, 3, 4, 5].map((level) => (
                    <i key={level} style={{ background: `var(--heat-${level})` }} />
                  ))}
                  <span>多</span>
                  <span>（最大 {maxCount}）</span>
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
              <ToolsTable tools={tools} metric={metric} />
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
