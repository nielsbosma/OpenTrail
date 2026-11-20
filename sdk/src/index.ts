import { getRecordConsolePlugin } from '@rrweb/rrweb-plugin-console-record';
import { record } from 'rrweb';
import { type Metric, onCLS, onFCP, onINP, onLCP, onTTFB } from 'web-vitals';
import { describe } from './describe';
import { watchNetwork } from './network';

export interface Options {
  /** The project's public key, `trk_pub_...`. */
  key: string;
  /** The OpenTrail server, e.g. `https://trail.example.com`. The script tag build defaults to where it was loaded from. */
  host?: string;
  /** Your app's version, to filter sessions by. */
  release?: string;
  /** Replace all text with asterisks unless inside `data-trail-unmask`. Inputs are always masked. */
  maskAllText?: boolean;
  /** Send request and response bodies (10 kB each). The project must also capture bodies. */
  captureBodies?: boolean;
  /** Record canvas contents. Costly; the project must also capture canvas. */
  captureCanvas?: boolean;
  /** Share of sessions to record, 0 to 1. */
  sampleRate?: number;
}

type Payload = Record<string, unknown>;

const SESSION_KEY = 'opentrail:session';
const WINDOW_KEY = 'opentrail:window';
const IDLE_MS = 30 * 60 * 1000;
const MAX_SESSION_MS = 4 * 60 * 60 * 1000;
const FLUSH_MS = 2000;
const MAX_BATCH_EVENTS = 500;
// Browsers refuse keepalive bodies over 64 kB.
const KEEPALIVE_LIMIT = 60 * 1024;

/** A version 7 UUID: time-ordered, so sessions sort by when they started. */
function uuidv7(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  let ms = Date.now();
  for (let i = 5; i >= 0; i--) {
    bytes[i] = ms % 256;
    ms = Math.floor(ms / 256);
  }
  bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x70;
  bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function read<T>(storage: Storage | undefined, key: string): T | undefined {
  try {
    const raw = storage?.getItem(key);
    return raw ? (JSON.parse(raw) as T) : undefined;
  } catch {
    return undefined;
  }
}

function write(storage: Storage | undefined, key: string, value: unknown): void {
  try {
    storage?.setItem(key, JSON.stringify(value));
  } catch {
    // Storage can be full or blocked; the recording still works for this page.
  }
}

function storage(kind: 'localStorage' | 'sessionStorage'): Storage | undefined {
  try {
    return window[kind];
  } catch {
    return undefined;
  }
}

interface SessionState {
  id: string;
  started: number;
  last: number;
  sampled: boolean;
}

interface WindowState {
  id: string;
  seq: number;
}

async function gzip(text: string): Promise<BodyInit> {
  if (typeof CompressionStream === 'undefined') return text;
  try {
    const stream = new Blob([text]).stream().pipeThrough(new CompressionStream('gzip'));
    return new Uint8Array(await new Response(stream).arrayBuffer());
  } catch {
    return text;
  }
}

class Recorder {
  private events: unknown[] = [];
  private session: SessionState;
  private window: WindowState;
  private stopRecording?: () => void;
  private cleanups: (() => void)[] = [];
  private timer?: ReturnType<typeof setInterval>;
  private pausedUntil = 0;
  private stopped = false;
  private sending = Promise.resolve();
  private described = new Set<number>();
  private lastUrl = '';
  // rrweb starts after the page's load event and throws on custom events before that.
  private queued: [string, Payload][] | undefined = [];
  private readonly endpoint: string;

  constructor(private readonly options: Options & { host: string }) {
    this.endpoint = `${options.host.replace(/\/$/, '')}/i/v1/batch?k=${encodeURIComponent(options.key)}`;
    this.session = this.currentSession();
    this.window = read<WindowState>(storage('sessionStorage'), WINDOW_KEY) ?? { id: uuidv7(), seq: 0 };
  }

  private currentSession(): SessionState {
    const now = Date.now();
    const saved = read<SessionState>(storage('localStorage'), SESSION_KEY);
    if (saved && now - saved.last < IDLE_MS && now - saved.started < MAX_SESSION_MS) return saved;
    const rate = this.options.sampleRate ?? 1;
    const fresh = { id: uuidv7(), started: now, last: now, sampled: Math.random() < rate };
    write(storage('localStorage'), SESSION_KEY, fresh);
    return fresh;
  }

  start(): boolean {
    if (!this.session.sampled) return false;
    const { maskAllText = false, captureBodies = false, captureCanvas = false } = this.options;
    this.stopRecording = record({
      emit: (event) => this.push(event),
      maskAllInputs: true,
      maskInputFn: (text, element) =>
        element?.closest('[data-trail-unmask]') && element.getAttribute('type') !== 'password' ? text : '*'.repeat(text.length),
      maskTextSelector: maskAllText ? '*' : '[data-trail-mask]',
      maskTextFn: (text, element) => (element?.closest('[data-trail-unmask]') ? text : text.replace(/\S/g, '*')),
      blockSelector: '[data-trail-block]',
      recordCanvas: captureCanvas,
      collectFonts: false,
      inlineImages: false,
      inlineStylesheet: true,
      slimDOMOptions: {
        script: true,
        comment: true,
        headFavicon: true,
        headMetaDescKeywords: true,
        headMetaSocial: true,
        headMetaRobots: true,
        headMetaHttpEquiv: true,
        headMetaAuthorship: true,
        headMetaVerification: true,
      },
      sampling: { mousemove: 50, scroll: 150, input: 'last' },
      plugins: [
        getRecordConsolePlugin({
          level: ['log', 'info', 'warn', 'error', 'debug', 'assert'],
          lengthThreshold: 1000,
          stringifyOptions: { stringLengthLimit: 1000, numOfKeysLimit: 50, depthOfLimit: 3 },
        }),
      ],
    });
    if (!this.stopRecording) return false;

    this.page();
    this.watchNavigation();
    this.watchElements(maskAllText);
    this.watchErrors();
    this.cleanups.push(
      watchNetwork(
        (r) => this.custom('trail/net', { ...r }),
        (url) => url.startsWith(this.endpoint.split('?')[0] ?? this.endpoint),
        captureBodies,
      ),
    );
    const vital = (m: Metric) => this.custom('trail/vital', { name: m.name, value: m.value });
    for (const on of [onLCP, onINP, onCLS, onFCP, onTTFB]) on(vital);

    this.timer = setInterval(() => void this.flush(false), FLUSH_MS);
    const hidden = () => {
      if (document.visibilityState === 'hidden') void this.flush(true);
    };
    const pagehide = () => void this.flush(true);
    document.addEventListener('visibilitychange', hidden);
    window.addEventListener('pagehide', pagehide);
    this.cleanups.push(() => {
      document.removeEventListener('visibilitychange', hidden);
      window.removeEventListener('pagehide', pagehide);
    });
    return true;
  }

  stop(): void {
    this.stopped = true;
    void this.flush(true);
    this.stopRecording?.();
    if (this.timer) clearInterval(this.timer);
    for (const cleanup of this.cleanups.splice(0)) cleanup();
  }

  get sessionId(): string {
    return this.session.id;
  }

  custom(tag: string, payload: Payload): void {
    if (this.stopped) return;
    if (this.queued) this.queued.push([tag, payload]);
    else record.addCustomEvent(tag, payload);
  }

  private push(event: unknown): void {
    const queued = this.queued;
    // Meta (4) comes from rrweb's start; DOMContentLoaded and Load come before it.
    if (queued && (event as { type?: number }).type === 4) {
      this.queued = undefined;
      queueMicrotask(() => {
        for (const [tag, payload] of queued) this.custom(tag, payload);
      });
    }
    const now = Date.now();
    if (now - this.session.last > IDLE_MS || now - this.session.started > MAX_SESSION_MS) {
      // A new session needs its own full snapshot, which rrweb takes on a checkout.
      void this.flush(false);
      this.session = this.currentSession();
      this.window = { id: uuidv7(), seq: 0 };
      this.described.clear();
      this.events.push(event);
      record.takeFullSnapshot(true);
      return;
    }
    this.events.push(event);
    if (this.events.length >= MAX_BATCH_EVENTS) void this.flush(false);
  }

  private page(): void {
    if (location.href === this.lastUrl) return;
    this.lastUrl = location.href;
    this.custom('trail/page', { url: location.href, title: document.title });
  }

  private watchNavigation(): void {
    const later = () => setTimeout(() => this.page(), 50);
    const { pushState, replaceState } = history;
    history.pushState = function trailPushState(...args: Parameters<History['pushState']>) {
      pushState.apply(this, args);
      later();
    };
    history.replaceState = function trailReplaceState(...args: Parameters<History['replaceState']>) {
      replaceState.apply(this, args);
      later();
    };
    window.addEventListener('popstate', later);
    window.addEventListener('hashchange', later);
    this.cleanups.push(() => {
      history.pushState = pushState;
      history.replaceState = replaceState;
      window.removeEventListener('popstate', later);
      window.removeEventListener('hashchange', later);
    });
  }

  /** Before rrweb sees a click or an input, say what the element is. */
  private watchElements(maskAllText: boolean): void {
    const onTouch = (event: Event) => {
      const target = event.target;
      if (!(target instanceof Element)) return;
      const id = record.mirror.getId(target);
      if (id < 0 || this.described.has(id)) return;
      this.described.add(id);
      this.custom('trail/el', { id, ...describe(target, maskAllText) });
    };
    for (const type of ['pointerdown', 'focusin', 'contextmenu'] as const) window.addEventListener(type, onTouch, true);
    this.cleanups.push(() => {
      for (const type of ['pointerdown', 'focusin', 'contextmenu'] as const) window.removeEventListener(type, onTouch, true);
    });
  }

  private watchErrors(): void {
    const onError = (event: ErrorEvent) => {
      const error = event.error as Error | undefined;
      this.custom('trail/error', {
        kind: 'error',
        message: error?.message ?? event.message,
        stack: error?.stack ?? `${event.filename}:${event.lineno}:${event.colno}`,
      });
    };
    const onRejection = (event: PromiseRejectionEvent) => {
      const reason = event.reason as { message?: string; stack?: string } | undefined;
      this.custom('trail/error', { kind: 'unhandledrejection', message: reason?.message ?? String(event.reason), stack: reason?.stack });
    };
    window.addEventListener('error', onError);
    window.addEventListener('unhandledrejection', onRejection);
    this.cleanups.push(() => {
      window.removeEventListener('error', onError);
      window.removeEventListener('unhandledrejection', onRejection);
    });
  }

  identify(userRef: string): void {
    this.custom('trail/identify', { userRef: String(userRef).slice(0, 200) });
  }

  private flush(leaving: boolean): Promise<void> {
    if (this.events.length === 0 || Date.now() < this.pausedUntil) return this.sending;
    const events = this.events.splice(0);
    const session = this.session;
    const window_ = this.window;
    const seq = window_.seq++;
    session.last = Date.now();
    write(storage('localStorage'), SESSION_KEY, session);
    write(storage('sessionStorage'), WINDOW_KEY, window_);
    const batch = JSON.stringify({
      v: 1,
      sessionId: session.id,
      windowId: window_.id,
      seq,
      release: this.options.release,
      lang: navigator.language,
      events,
    });
    if (leaving) {
      // The page can be gone before an async gzip finishes, so this one starts now, uncompressed.
      const body = new TextEncoder().encode(batch);
      void this.post(body, body.byteLength < KEEPALIVE_LIMIT);
      return this.sending;
    }
    this.sending = this.sending.then(async () => this.post(await gzip(batch), false));
    return this.sending;
  }

  private async post(body: BodyInit, keepalive: boolean, attempt = 0): Promise<void> {
    try {
      const response = await fetch(this.endpoint, { method: 'POST', body, keepalive, credentials: 'omit', mode: 'cors' });
      if (response.status === 429) {
        this.pausedUntil = Date.now() + (Number(response.headers.get('retry-after')) || 60) * 1000;
      } else if (response.status === 401 || response.status === 403) {
        this.stop();
      } else if (response.status >= 500 && attempt < 3) {
        await new Promise((r) => setTimeout(r, 1000 * 2 ** attempt));
        return this.post(body, keepalive, attempt + 1);
      }
    } catch {
      if (attempt < 3 && !keepalive) {
        await new Promise((r) => setTimeout(r, 1000 * 2 ** attempt));
        return this.post(body, keepalive, attempt + 1);
      }
    }
  }
}

let current: Recorder | undefined;

/** Starts recording. Returns false when this session was not sampled or the browser cannot record. */
export function init(options: Options): boolean {
  if (current || typeof window === 'undefined') return false;
  if (!options.key || !options.host) throw new Error('OpenTrail needs a key and a host');
  current = new Recorder({ ...options, host: options.host });
  if (!current.start()) {
    current = undefined;
    return false;
  }
  return true;
}

/** Ties the session to your own id for the user (never an email or name). */
export function identify(userRef: string): void {
  current?.identify(userRef);
}

/** Records a named event with optional properties (at most 4 kB). */
export function track(name: string, props?: Payload): void {
  current?.custom('trail/custom', { name, props });
}

export function stop(): void {
  current?.stop();
  current = undefined;
}

export function sessionId(): string | undefined {
  return current?.sessionId;
}

// The script tag build starts itself: <script src="https://trail.example.com/sdk.js" data-key="trk_pub_..."></script>
const script = typeof document !== 'undefined' ? (document.currentScript as HTMLScriptElement | null) : null;
if (script?.dataset.key) {
  const d = script.dataset;
  init({
    key: d.key ?? '',
    host: d.host ?? new URL(script.src).origin,
    release: d.release,
    maskAllText: d.maskAllText === 'true',
    captureBodies: d.captureBodies === 'true',
    captureCanvas: d.captureCanvas === 'true',
    sampleRate: d.sampleRate ? Number(d.sampleRate) : undefined,
  });
}
