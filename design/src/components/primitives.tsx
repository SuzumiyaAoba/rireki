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
  CircleAlert,
  Clock,
  Copy,
  ExternalLink,
  FilePen,
  FileText,
  Filter,
  Folder,
  GitBranch,
  Globe,
  Hourglass,
  Keyboard,
  Layers,
  ListChecks,
  ListTree,
  LoaderCircle,
  LocateFixed,
  Maximize,
  MessageSquare,
  Minimize,
  PanelLeftClose,
  PanelLeftOpen,
  Pause,
  Play,
  Plug,
  Search,
  SlidersHorizontal,
  Terminal,
  User,
  Wrench,
  X,
  ZoomIn,
  ZoomOut,
} from 'lucide-react';
import claudeLogo from '../../../assets/icons/claude.svg';
import codexLogo from '../../../assets/icons/codex.svg';
import appLogo from '../../../assets/app-icon/rireki.svg';
import { eventKinds, type EventKind } from '../tokens';
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
  CircleAlert,
  Clock,
  Copy,
  ExternalLink,
  FilePen,
  FileText,
  Filter,
  Folder,
  GitBranch,
  Globe,
  Hourglass,
  Keyboard,
  Layers,
  ListChecks,
  ListTree,
  LoaderCircle,
  LocateFixed,
  Maximize,
  MessageSquare,
  Minimize,
  PanelLeftClose,
  PanelLeftOpen,
  Pause,
  Play,
  Plug,
  Search,
  SlidersHorizontal,
  Terminal,
  User,
  Wrench,
  X,
  ZoomIn,
  ZoomOut,
};

export type IconName = keyof typeof icons;

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
  return <Glyph size={size} strokeWidth={1.6} aria-hidden="true" className={`icon ${className}`} />;
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

// 状態は色付きの文字で示す。進行中だけは点を添える。
export function StatusText({
  children,
  tone = 'muted',
}: {
  children: ReactNode;
  tone?: 'live' | 'err' | 'warn' | 'muted';
}) {
  return <span className={`status-text ${tone}`}>{children}</span>;
}

export function Segmented<T extends string>({
  label,
  options,
  value,
  onChange,
}: {
  label: string;
  options: ReadonlyArray<{ value: T; label: string }>;
  value: T;
  onChange: (value: T) => void;
}) {
  return (
    <div className="segmented" role="group" aria-label={label}>
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
  selected = false,
  onClick,
  style,
}: {
  kind: EventKind;
  label: string;
  failed?: boolean;
  selected?: boolean;
  onClick?: () => void;
  style?: CSSProperties;
}) {
  const token = eventKinds[kind];
  return (
    <button
      type="button"
      className={`event-block ${failed ? 'failed' : ''} ${selected ? 'selected' : ''}`}
      aria-pressed={selected}
      title={label}
      onClick={onClick}
      style={{ '--event-color': token.color, ...style } as CSSProperties}
    >
      <Icon name={token.icon} size={11} />
      <span>{label}</span>
      {failed && <Icon name="CircleAlert" size={11} />}
    </button>
  );
}

export function EmptyState({
  loading = false,
  filtered = false,
  onReset,
}: {
  loading?: boolean;
  filtered?: boolean;
  onReset?: () => void;
}) {
  return (
    <div className="empty-state" role="status">
      <span>
        {loading ? '読み込み中…' : filtered ? '一致する履歴はありません' : 'まだ履歴がありません'}
      </span>
      {filtered && onReset && <Button onClick={onReset}>絞り込みを解除</Button>}
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
