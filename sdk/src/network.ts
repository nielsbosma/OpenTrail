/** One finished request, as `trail/net` sends it. Headers are never recorded. */
export interface Request {
  initiator: 'fetch' | 'xhr';
  method: string;
  url: string;
  /** 0 when there was no response. */
  status: number;
  durationMs: number;
  bytes?: number;
  requestBody?: string;
  responseBody?: string;
}

const BODY_LIMIT = 10 * 1024;
const TEXTUAL = /json|text|xml|x-www-form-urlencoded|graphql/i;

function cut(text: string): string {
  return text.length > BODY_LIMIT ? `${text.slice(0, BODY_LIMIT)}…` : text;
}

function bodyText(body: unknown): string | undefined {
  if (typeof body === 'string') return cut(body);
  if (body instanceof URLSearchParams) return cut(body.toString());
  return undefined;
}

function absolute(url: string): string {
  try {
    return new URL(url, location.href).href;
  } catch {
    return url;
  }
}

function lengthOf(headers: Headers | null | undefined): number | undefined {
  const value = headers?.get('content-length');
  return value ? Number(value) || undefined : undefined;
}

/** Wraps fetch and XMLHttpRequest. Returns a function that puts them back. */
export function watchNetwork(emit: (r: Request) => void, ignore: (url: string) => boolean, bodies: boolean): () => void {
  const originalFetch = window.fetch;
  const started = () => performance.now();

  window.fetch = async function trailFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
    const url = absolute(input instanceof Request ? input.url : String(input));
    if (ignore(url)) return originalFetch.call(window, input, init);
    const method = (init?.method ?? (input instanceof Request ? input.method : 'GET')).toUpperCase();
    const t0 = started();
    const requestBody = bodies ? bodyText(init?.body) : undefined;
    try {
      const response = await originalFetch.call(window, input, init);
      const done: Request = {
        initiator: 'fetch',
        method,
        url,
        status: response.status,
        durationMs: Math.round(started() - t0),
        bytes: lengthOf(response.headers),
        requestBody,
      };
      if (bodies && TEXTUAL.test(response.headers.get('content-type') ?? '')) {
        response
          .clone()
          .text()
          .then((text) => emit({ ...done, responseBody: cut(text) }))
          .catch(() => emit(done));
      } else {
        emit(done);
      }
      return response;
    } catch (error) {
      emit({ initiator: 'fetch', method, url, status: 0, durationMs: Math.round(started() - t0), requestBody });
      throw error;
    }
  };

  const proto = XMLHttpRequest.prototype;
  const open = proto.open;
  const send = proto.send;
  const state = new WeakMap<XMLHttpRequest, { method: string; url: string }>();
  proto.open = function trailOpen(this: XMLHttpRequest, method: string, url: string | URL, ...rest: unknown[]) {
    state.set(this, { method: method.toUpperCase(), url: absolute(String(url)) });
    // biome-ignore lint/suspicious/noExplicitAny: open has overloads that a spread cannot name.
    return (open as any).call(this, method, url, ...rest);
  };
  proto.send = function trailSend(this: XMLHttpRequest, body?: Document | XMLHttpRequestBodyInit | null) {
    const info = state.get(this);
    if (info && !ignore(info.url)) {
      const t0 = started();
      const requestBody = bodies ? bodyText(body) : undefined;
      this.addEventListener('loadend', () => {
        const textual = this.responseType === '' || this.responseType === 'text';
        emit({
          initiator: 'xhr',
          method: info.method,
          url: info.url,
          status: this.status,
          durationMs: Math.round(started() - t0),
          bytes: Number(this.getResponseHeader('content-length')) || undefined,
          requestBody,
          responseBody:
            bodies && textual && TEXTUAL.test(this.getResponseHeader('content-type') ?? '') ? cut(this.responseText) : undefined,
        });
      });
    }
    return send.call(this, body);
  };

  return () => {
    window.fetch = originalFetch;
    proto.open = open;
    proto.send = send;
  };
}
