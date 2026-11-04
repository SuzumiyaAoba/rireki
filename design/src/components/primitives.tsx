import { useState, type ButtonHTMLAttributes, type CSSProperties, type ReactNode } from 'react';
import {
  Activity,
  ArrowDown,
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  Bot,
  Brain,
  Calendar,
  ChartColumn,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Clock,
  CloudDownload,
  Copy,
  ExternalLink,
  FilePen,
  FilePlus,
  FileText,
  Filter,
  Folder,
  FolderSearch,
  GitBranch,
  Globe,
  Hourglass,
  Image as ImageIcon,
  Keyboard,
  Layers,
  ListChecks,
  ListTree,
  LoaderCircle,
  LocateFixed,
  Map as MapIcon,
  Maximize,
  MessageSquare,
  Minimize,
  NotebookPen,
  PanelLeftClose,
  PanelLeftOpen,
  Pause,
  Play,
  Plug,
  Search,
  SlidersHorizontal,
  Sparkles,
  Target,
  Terminal,
  TextSearch,
  User,
  Wrench,
  X,
  ZoomIn,
  ZoomOut,
} from 'lucide-react';
import claudeLogo from '../../../assets/icons/claude.svg';
import codexLogo from '../../../assets/icons/codex.svg';
import appLogo from '../../../assets/app-icon/rireki.svg';
import { eventKinds, toolIcons, type EventKind } from '../tokens';
import { projectColor, type Source } from '../data';

const icons = {
  Activity,
  ArrowDown,
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  Bot,
  Brain,
  Calendar,
  ChartColumn,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Clock,
  CloudDownload,
  Copy,
  ExternalLink,
  FilePen,
  FilePlus,
  FileText,
  Filter,
  Folder,
  FolderSearch,
  GitBranch,
  Globe,
  Hourglass,
  Image: ImageIcon,
  Keyboard,
  Layers,
  ListChecks,
  ListTree,
  LoaderCircle,
  LocateFixed,
  Map: MapIcon,
  Maximize,
  MessageSquare,
  Minimize,
  NotebookPen,
  PanelLeftClose,
  PanelLeftOpen,
  Pause,
  Play,
  Plug,
  Search,
  SlidersHorizontal,
  Sparkles,
  Target,
  Terminal,
  TextSearch,
  User,
  Wrench,
  X,
  ZoomIn,
  ZoomOut,
};

export type IconName = keyof typeof icons;

// toolIcons の値がすべて登録済みのアイコンであることを型で確かめる
toolIcons satisfies Record<string, IconName>;

/** イベントのアイコン。ツール名ごとの指定を優先し、なければ種類の既定を使う */
export function eventIcon(name: string, kind: EventKind): IconName {
  // ユーザー・応答・思考・サブエージェントは種類のアイコン。ツールはツール名ごとで、未登録は工具
  if (['user', 'assistant', 'thinking', 'agent'].includes(kind)) return eventKinds[kind].icon;
  if (name.startsWith('mcp__')) return 'Plug';
  return (toolIcons as Record<string, IconName>)[name] ?? 'Wrench';
}

export function Icon({
  name,
  size = 14,
  className = '',
}: {
  name: IconName;
  size?: number;
  className?: string;
}) {
  const Glyph = icons[name];
  // 線の太さは src/icons.rs の SVG（stroke-width="2"）と同じ
  return <Glyph size={size} strokeWidth={2} aria-hidden="true" className={`icon ${className}`} />;
}

export function IconButton({
  icon,
  label,
  active = false,
  className = '',
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { icon: IconName; label: string; active?: boolean }) {
  return (
    <button
      type="button"
      className={`icon-button ${active ? 'active' : ''} ${className}`}
      aria-label={label}
      title={label}
      {...props}
    >
      <Icon name={icon} />
    </button>
  );
}

export function Button({
  icon,
  children,
  className = '',
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { icon?: IconName; children: ReactNode }) {
  return (
    <button type="button" className={`button ${className}`} {...props}>
      {icon && <Icon name={icon} />}
      {children}
    </button>
  );
}

export function SourceLogo({ source, size = 14 }: { source: Source; size?: number }) {
  if (source === 'all') return <img src={appLogo} alt="rireki" width={size} height={size} />;
  return (
    <span
      aria-label={source === 'claude' ? 'Claude Code' : 'Codex'}
      role="img"
      className={`source-logo ${source}`}
      style={{
        width: size,
        height: size,
        maskImage: `url("${source === 'claude' ? claudeLogo : codexLogo}")`,
      }}
    />
  );
}

export function ProjectMark({ name }: { name: string }) {
  return (
    <i aria-hidden="true" className="project-mark" style={{ background: projectColor(name) }} />
  );
}

// 状態は色付きの文字で示す。進行中だけは点を添える。実行中は色の文字だけ（点なし）。
export function StatusText({
  children,
  tone = 'muted',
}: {
  children: ReactNode;
  tone?: 'live' | 'running' | 'err' | 'warn' | 'muted';
}) {
  return <span className={`status-text ${tone}`}>{children}</span>;
}

// 詳細や統計の内側の切り替え（primary-bg の選択）と、ツールバーの切り替え（枠・raised の選択）
export function Segmented<T extends string>({
  label,
  options,
  value,
  onChange,
  toolbar = false,
}: {
  label: string;
  options: ReadonlyArray<{ value: T; label: string }>;
  value: T;
  onChange: (value: T) => void;
  toolbar?: boolean;
}) {
  return (
    <div className={`segmented${toolbar ? ' toolbar' : ''}`} role="group" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          aria-pressed={option.value === value}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

export function CopyButton({ text, label = 'コピー' }: { text: string; label?: string }) {
  const [status, setStatus] = useState<'idle' | 'copied' | 'error'>('idle');
  return (
    <Button
      icon={status === 'copied' ? 'Check' : 'Copy'}
      className="copy-button"
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(text);
          setStatus('copied');
        } catch {
          setStatus('error');
        }
      }}
    >
      {status === 'copied'
        ? 'コピーしました'
        : status === 'error'
          ? 'コピーできませんでした'
          : label}
    </Button>
  );
}

// ラベルと値を 2 列に並べる。失敗など注意の要る値だけ tone で色を付ける。
export function Facts({
  items,
}: {
  items: Array<{ label: string; value: ReactNode; tone?: 'err'; hint?: string }>;
}) {
  return (
    <dl className="facts">
      {items.map((item) => (
        <div key={item.label} title={item.hint}>
          <dt>{item.label}</dt>
          <dd className={item.tone}>{item.value}</dd>
        </div>
      ))}
    </dl>
  );
}

export function EventBlock({
  kind,
  label,
  failed = false,
  rejected = false,
  icon,
  selected = false,
  onClick,
  style,
}: {
  kind: EventKind;
  label: string;
  failed?: boolean;
  rejected?: boolean;
  icon?: IconName;
  selected?: boolean;
  onClick?: () => void;
  style?: CSSProperties;
}) {
  const token = eventKinds[kind];
  // 失敗・拒否・中断は種類のアイコンを X に替える（src/app/timeline.rs の block_paint と同じ）
  const outcome = failed || rejected;
  return (
    <button
      type="button"
      className={`event-block ${failed ? 'failed' : rejected ? 'rejected' : ''} ${selected ? 'selected' : ''}`}
      aria-pressed={selected}
      title={label}
      onClick={onClick}
      style={{ '--event-color': token.color, ...style } as CSSProperties}
    >
      <Icon name={outcome ? 'X' : (icon ?? token.icon)} size={11} />
      <span>{label}</span>
    </button>
  );
}

// 空状態は 1 行の文言と、必要なら移動の操作が 1 つ。説明の 2 行目は書かない。
export function EmptyState({
  message,
  actionLabel,
  onAction,
}: {
  message: string;
  // 足してよい操作は、空の期間から最新の作業へ移る 1 つだけ
  actionLabel?: string;
  onAction?: () => void;
}) {
  return (
    <div className="empty-state">
      <span role="status">{message}</span>
      {actionLabel && onAction && (
        <Button icon="Clock" onClick={onAction}>
          {actionLabel}
        </Button>
      )}
    </div>
  );
}

export function Highlight({ text, query }: { text: string; query: string }) {
  const terms = query.trim().split(/\s+/).filter(Boolean);
  if (!terms.length) return <>{text}</>;
  const escaped = terms.map((term) => term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  const pattern = new RegExp(`(${escaped.join('|')})`, 'gi');
  return (
    <>
      {text
        .split(pattern)
        .map((part, index) =>
          terms.some((term) => term.toLowerCase() === part.toLowerCase()) ? (
            <mark key={index}>{part}</mark>
          ) : (
            part
          ),
        )}
    </>
  );
}
