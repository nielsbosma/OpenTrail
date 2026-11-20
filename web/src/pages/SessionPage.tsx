import { ArrowLeft, CircleAlert, FastForward, MousePointerClick, Pause, Play, Trash2 } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { Replayer } from 'rrweb';
import 'rrweb/dist/style.css';
import { ConfirmButton, EmptyState, Loading, PageHeader } from '@/components/common';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { call, type EventRow, useApi, useApiMutation } from '@/lib/api';
import { formatBytes, formatDateTime } from '@/lib/format';
import { t } from '@/lib/i18n';
import { useSession } from '@/lib/session';
import { cn } from '@/lib/utils';
import { formatDuration } from './SessionsPage';

type ReplayEvent = Exclude<ConstructorParameters<typeof Replayer>[0][number], string>;
type Data = Record<string, unknown>;

/** Every page of one tab's recording. */
function useRecording(sessionId: string, windowId?: string) {
  const [state, setState] = useState<{ events?: ReplayEvent[]; error?: string }>({});
  useEffect(() => {
    if (!windowId) return;
    let cancelled = false;
    setState({});
    (async () => {
      const events: ReplayEvent[] = [];
      let afterSeq: number | undefined;
      for (;;) {
        const page = await call('sessions.replay', { id: sessionId, windowId, afterSeq });
        events.push(...(page.events as ReplayEvent[]));
        if (page.nextSeq == null || cancelled) break;
        afterSeq = page.nextSeq;
      }
      if (!cancelled) setState({ events });
    })().catch((e: unknown) => !cancelled && setState({ error: e instanceof Error ? e.message : String(e) }));
    return () => {
      cancelled = true;
    };
  }, [sessionId, windowId]);
  return state;
}

/** All timeline pages of one session. */
function useTimeline(sessionId: string) {
  const [items, setItems] = useState<EventRow[]>();
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const all: EventRow[] = [];
      let after: string | undefined;
      for (;;) {
        const page = await call('sessions.timeline', { id: sessionId, after, limit: 5000 });
        all.push(...page.items);
        if (!page.next || cancelled) break;
        after = page.next;
      }
      if (!cancelled) setItems(all);
    })().catch(() => !cancelled && setItems([]));
    return () => {
      cancelled = true;
    };
  }, [sessionId]);
  return items;
}

const SPEEDS = [1, 2, 4, 8];

function Player({ events, onTime, seekTo }: { events: ReplayEvent[]; onTime: (ms: number) => void; seekTo?: { ms: number; n: number } }) {
  const host = useRef<HTMLDivElement>(null);
  const frame = useRef<HTMLDivElement>(null);
  const replayer = useRef<Replayer | null>(null);
  const [playing, setPlaying] = useState(false);
  const [time, setTime] = useState(0);
  const [speed, setSpeed] = useState(1);
  const [skip, setSkip] = useState(true);
  const [size, setSize] = useState({ width: 1280, height: 720 });
  const total = useMemo(() => {
    const first = events[0]?.timestamp ?? 0;
    const last = events.at(-1)?.timestamp ?? first;
    return last - first;
  }, [events]);

  useEffect(() => {
    if (!frame.current || events.length < 2) return;
    const r = new Replayer(events, {
      root: frame.current,
      skipInactive: true,
      showWarning: false,
      mouseTail: { strokeStyle: 'rgba(79, 70, 229, 0.6)', lineWidth: 2 },
      UNSAFE_replayCanvas: false,
    });
    replayer.current = r;
    r.on('resize', (d) => setSize(d as { width: number; height: number }));
    r.on('finish', () => setPlaying(false));
    const meta = events.find((e) => e.type === 4) as { data?: { width?: number; height?: number } } | undefined;
    if (meta?.data?.width && meta.data.height) setSize({ width: meta.data.width, height: meta.data.height });
    r.pause(0);
    const tick = setInterval(() => {
      const now = r.getCurrentTime();
      setTime(now);
      onTime(now);
    }, 200);
    return () => {
      clearInterval(tick);
      r.destroy();
      replayer.current = null;
    };
  }, [events, onTime]);

  useEffect(() => {
    if (!seekTo || !replayer.current) return;
    replayer.current.pause(Math.max(0, seekTo.ms));
    setPlaying(false);
    setTime(seekTo.ms);
  }, [seekTo]);

  const [scale, setScale] = useState(1);
  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const resize = () => setScale(Math.min(1, el.clientWidth / size.width, 560 / size.height));
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(el);
    return () => observer.disconnect();
  }, [size]);

  const toggle = () => {
    const r = replayer.current;
    if (!r) return;
    if (playing) r.pause();
    else r.play(time >= total ? 0 : time);
    setPlaying(!playing);
  };

  if (events.length < 2) return <EmptyState title={t('Nothing to play yet')}>{t('This tab sent too little to replay.')}</EmptyState>;
  return (
    <div className="grid gap-2">
      <div ref={host} className="overflow-hidden rounded-lg border bg-muted/40" style={{ height: size.height * scale }}>
        <div
          ref={frame}
          className="trail-player origin-top-left"
          style={{ transform: `scale(${scale})`, width: size.width, height: size.height }}
        />
      </div>
      <div className="flex items-center gap-3">
        <Button variant="outline" size="icon" onClick={toggle} aria-label={playing ? t('Pause') : t('Play')}>
          {playing ? <Pause /> : <Play />}
        </Button>
        <input
          type="range"
          min={0}
          max={total}
          value={Math.min(time, total)}
          onChange={(e) => {
            const ms = Number(e.target.value);
            setTime(ms);
            if (playing) replayer.current?.play(ms);
            else replayer.current?.pause(ms);
          }}
          className="flex-1 accent-primary"
          aria-label={t('Position')}
        />
        <span className="w-24 text-end text-xs tabular-nums text-muted-foreground">
          {formatDuration(Math.floor(time / 1000))} / {formatDuration(Math.floor(total / 1000))}
        </span>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            const next = SPEEDS[(SPEEDS.indexOf(speed) + 1) % SPEEDS.length] ?? 1;
            setSpeed(next);
            replayer.current?.setConfig({ speed: next });
          }}
          aria-label={t('Speed')}
        >
          {speed}×
        </Button>
        <Button
          variant={skip ? 'secondary' : 'ghost'}
          size="sm"
          onClick={() => {
            setSkip(!skip);
            replayer.current?.setConfig({ skipInactive: !skip });
          }}
        >
          <FastForward /> {t('Skip Idle')}
        </Button>
      </div>
    </div>
  );
}

function describe(e: EventRow): string {
  const d = e.data as Data;
  switch (e.kind) {
    case 'page':
      return String(d.url ?? '');
    case 'interaction': {
      const what = [d.role, d.name ? `“${d.name}”` : undefined, d.testid ? `[${d.testid}]` : undefined].filter(Boolean).join(' ');
      const length = d.inputLength != null ? ` · ${t('{n} characters', { n: String(d.inputLength) })}` : '';
      return `${d.kind} ${what || `#${d.nodeId ?? '?'}`}${d.component ? ` · ${d.component}` : ''}${length}`;
    }
    case 'console':
      return String(d.message ?? '');
    case 'network':
      return `${d.method} ${d.url} → ${d.status === 0 ? t('failed') : d.status} · ${d.durationMs ?? '?'} ms`;
    case 'error':
      return String(d.message ?? '');
    case 'vital':
      return `${d.name} ${Number(d.value).toFixed(d.name === 'CLS' ? 3 : 0)}`;
    case 'custom':
      return `${d.name}${d.props ? ` ${JSON.stringify(d.props)}` : ''}`;
    default:
      return '';
  }
}

function tone(e: EventRow) {
  const d = e.data as Data;
  if (e.kind === 'error' || (e.kind === 'console' && d.level === 'error')) return 'text-destructive';
  if ((e.kind === 'network' && (Number(d.status) === 0 || Number(d.status) >= 400)) || (e.kind === 'console' && d.level === 'warn'))
    return 'text-warning';
  return '';
}

function EventList({
  items,
  start,
  now,
  onSeek,
  onOpen,
}: {
  items: EventRow[];
  start: number;
  now: number;
  onSeek: (ms: number) => void;
  onOpen?: (e: EventRow) => void;
}) {
  if (!items.length) return <p className="py-6 text-center text-muted-foreground">{t('Nothing of this kind in this session.')}</p>;
  return (
    <div className="max-h-[560px] overflow-auto rounded-md border">
      {items.map((e, i) => {
        const at = new Date(e.ts).getTime() - start;
        const d = e.data as Data;
        return (
          <button
            type="button"
            key={i}
            onClick={() => (onOpen && e.kind === 'network' && (d.hasRequestBody || d.hasResponseBody) ? onOpen(e) : onSeek(at))}
            className={cn(
              'flex w-full items-start gap-2 border-b px-3 py-1.5 text-start text-xs last:border-b-0 hover:bg-accent',
              at > now && 'opacity-50',
            )}
          >
            <span className="w-12 shrink-0 tabular-nums text-muted-foreground">{formatDuration(Math.max(0, Math.floor(at / 1000)))}</span>
            <Badge variant="outline" className="shrink-0 font-normal">
              {e.kind === 'console' ? String(d.level) : e.kind}
            </Badge>
            <span className={cn('min-w-0 flex-1 break-words font-mono', tone(e))}>{describe(e)}</span>
          </button>
        );
      })}
    </div>
  );
}

function BodyDialog({ event, onClose }: { event: EventRow | null; onClose: () => void }) {
  const id = event ? Number((event.data as Data).id) : 0;
  const bodies = useApi('sessions.request_body', { id }, { enabled: !!event });
  return (
    <Dialog open={!!event} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>{t('Request Body')}</DialogTitle>
        </DialogHeader>
        {event && <p className="break-all font-mono text-xs">{describe(event)}</p>}
        {bodies.isPending ? (
          <Loading />
        ) : (
          <div className="grid gap-3">
            {(['requestBody', 'responseBody'] as const).map((k) => (
              <div key={k} className="grid gap-1">
                <div className="text-xs font-medium">{k === 'requestBody' ? t('Request') : t('Response')}</div>
                <pre className="max-h-64 overflow-auto rounded-md bg-muted p-2 text-xs whitespace-pre-wrap break-all">
                  {bodies.data?.[k] ?? t('Not recorded')}
                </pre>
              </div>
            ))}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

export function SessionPage() {
  const { id = '' } = useParams();
  const { can } = useSession();
  const navigate = useNavigate();
  const detail = useApi('sessions.get', { id });
  const [windowId, setWindowId] = useState<string>();
  const firstWindow = detail.data?.windows[0]?.windowId;
  const activeWindow = windowId ?? firstWindow;
  const recording = useRecording(id, activeWindow);
  const timeline = useTimeline(id);
  const [now, setNow] = useState(0);
  const [seekTo, setSeekTo] = useState<{ ms: number; n: number }>();
  const [body, setBody] = useState<EventRow | null>(null);
  const remove = useApiMutation('sessions.delete', { invalidates: ['sessions.'], onSuccess: () => navigate('/sessions') });

  const firstTs = detail.data?.windows.find((w) => w.windowId === activeWindow)?.firstTs;
  const start = recording.events?.[0]?.timestamp ?? (firstTs ? new Date(firstTs).getTime() : 0);
  const inWindow = useMemo(() => (timeline ?? []).filter((e) => e.windowId === activeWindow), [timeline, activeWindow]);
  const seek = (ms: number) => setSeekTo((s) => ({ ms, n: (s?.n ?? 0) + 1 }));
  const kinds = {
    events: inWindow.filter((e) => e.kind === 'page' || e.kind === 'interaction' || e.kind === 'custom'),
    console: inWindow.filter((e) => e.kind === 'console'),
    network: inWindow.filter((e) => e.kind === 'network'),
    errors: inWindow.filter((e) => e.kind === 'error'),
    vitals: inWindow.filter((e) => e.kind === 'vital'),
  };

  if (detail.isPending) return <Loading />;
  if (!detail.data) return <EmptyState title={t('Session not found')} />;
  const s = detail.data;

  return (
    <>
      <PageHeader
        title={formatDateTime(s.startedAt)}
        description={[
          s.projectName,
          s.userRef,
          [s.browser, s.os].filter(Boolean).join(' · '),
          formatDuration(s.durationSeconds),
          formatBytes(s.bytes),
        ]
          .filter(Boolean)
          .join(' — ')}
        actions={
          <>
            <Button variant="outline" asChild>
              <Link to="/sessions">
                <ArrowLeft /> {t('All Sessions')}
              </Link>
            </Button>
            {can('sessions.delete') && (
              <ConfirmButton
                label={t('Delete Session')}
                icon={<Trash2 />}
                title={t('Delete this session?')}
                description={t('The recording and everything extracted from it are deleted. This cannot be undone.')}
                onConfirm={() => remove.mutate({ id })}
              />
            )}
          </>
        }
      />
      {s.truncated && (
        <p className="mb-4 rounded-md border border-warning/40 bg-warning/10 px-3 py-2 text-sm">
          {t('This recording stopped early: the session ran over 4 hours or 100 MB.')}
        </p>
      )}
      <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_420px]">
        <div className="grid content-start gap-3">
          {s.windows.length > 1 && (
            <Select value={activeWindow} onValueChange={setWindowId}>
              <SelectTrigger className="w-64" aria-label={t('Tab')}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {s.windows.map((w, i) => (
                  <SelectItem key={w.windowId} value={w.windowId}>
                    {t('Tab {n}', { n: String(i + 1) })} · {formatDateTime(w.firstTs)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          {recording.error ? (
            <EmptyState icon={<CircleAlert />} title={t('The recording could not be loaded')}>
              {recording.error}
            </EmptyState>
          ) : recording.events ? (
            <Player events={recording.events} onTime={setNow} seekTo={seekTo} />
          ) : (
            <Loading />
          )}
          <div className="flex flex-wrap gap-2 text-xs text-muted-foreground">
            {s.pages.map((p, i) => (
              <button
                type="button"
                key={i}
                className="max-w-80 truncate rounded border px-2 py-0.5 font-mono hover:bg-accent"
                onClick={() => p.windowId === activeWindow && seek(new Date(p.ts).getTime() - start)}
                title={p.url}
              >
                {p.title || p.url}
              </button>
            ))}
          </div>
        </div>
        <Tabs defaultValue="events" className="min-w-0">
          <TabsList>
            <TabsTrigger value="events">
              <MousePointerClick /> {t('Events')}
            </TabsTrigger>
            <TabsTrigger value="console">
              {t('Console')}{' '}
              {kinds.console.length > 0 && <span className="tabular-nums text-muted-foreground">{kinds.console.length}</span>}
            </TabsTrigger>
            <TabsTrigger value="network">
              {t('Network')}{' '}
              {kinds.network.length > 0 && <span className="tabular-nums text-muted-foreground">{kinds.network.length}</span>}
            </TabsTrigger>
            <TabsTrigger value="errors">
              {t('Errors')} {kinds.errors.length > 0 && <span className="tabular-nums text-destructive">{kinds.errors.length}</span>}
            </TabsTrigger>
            <TabsTrigger value="vitals">{t('Vitals')}</TabsTrigger>
          </TabsList>
          {timeline === undefined ? (
            <Loading />
          ) : (
            (Object.keys(kinds) as (keyof typeof kinds)[]).map((k) => (
              <TabsContent key={k} value={k}>
                <EventList items={kinds[k]} start={start} now={now} onSeek={seek} onOpen={setBody} />
              </TabsContent>
            ))
          )}
        </Tabs>
      </div>
      <BodyDialog event={body} onClose={() => setBody(null)} />
    </>
  );
}
