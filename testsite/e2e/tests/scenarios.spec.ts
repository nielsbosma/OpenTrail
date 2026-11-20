import { expect, type Page, test } from '@playwright/test';

// OPENTRAIL_URL is how this runner and its browser reach OpenTrail; OPENTRAIL_ORIGIN is its
// APP_URL, which signed-in calls must name as their Origin.
const OT = process.env.OPENTRAIL_URL ?? 'http://localhost:8091';
const ORIGIN = process.env.OPENTRAIL_ORIGIN ?? OT;
const SITE = process.env.SITE_URL ?? 'http://localhost:8092';
const SETUP_CODE = process.env.SETUP_CODE ?? '';
const EMAIL = process.env.E2E_EMAIL ?? 'e2e-admin@example.com';
const PASSWORD = process.env.E2E_PASSWORD ?? 'e2e correct horse battery staple';

type Json = Record<string, unknown>;
interface Event {
  kind: string;
  windowId: string;
  ts: string;
  data: Json;
}

class Api {
  private cookie = '';

  async call<T = Json>(name: string, input: Json = {}): Promise<T> {
    const response = await fetch(`${OT}/api/rpc/${name}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', origin: ORIGIN, cookie: this.cookie },
      body: JSON.stringify(input),
    });
    const cookies = response.headers.getSetCookie();
    if (cookies.length > 0) this.cookie = cookies.map((c) => c.split(';')[0]).join('; ');
    const body = (await response.json()) as T;
    if (!response.ok) throw new Error(`${name} answered ${response.status}: ${JSON.stringify(body)}`);
    return body;
  }
}

const api = new Api();
let key = '';
let projectId = '';
let sessionId = '';

test.beforeAll(async () => {
  const status = await api.call<{ needsSetup: boolean }>('setup.status');
  if (status.needsSetup) {
    await api.call('setup.create_first_admin', {
      setupCode: SETUP_CODE,
      orgName: 'E2E',
      name: 'E2E Admin',
      email: EMAIL,
      password: PASSWORD,
    });
  }
  const login = await api.call<{ status: string }>('auth.login', { email: EMAIL, password: PASSWORD });
  expect(login.status).toBe('signed_in');
  const project = await api.call<{ id: string; keys: { key: string }[] }>('projects.create', {
    name: `Test Site ${new Date().toISOString()}`,
    allowedOrigins: [SITE],
  });
  projectId = project.id;
  key = project.keys[0]?.key ?? '';
  await api.call('projects.update', { id: projectId, captureBodies: true });
});

async function interact(page: Page) {
  await page.click('#pay');
  await page.click('#details');
  await page.dblclick('#double');
  await page.click('#menu', { button: 'right' });
  await page.click('#danger');
  await page.fill('#name', 'Ada Lovelace');
  await page.type('#pnr', '19811218-9876');
  await page.type('#card', '4111 1111 1111 1111');
  await page.type('#password', 'hunter2-secret');
  await page.type('#unmasked', 'visible search');
  await page.check('#agree');
  await page.selectOption('#size', 'Large');
  await page.frameLocator('#frame').locator('#frame-button').click();
}

async function timeline(): Promise<Event[]> {
  const items: Event[] = [];
  let after: string | undefined;
  for (;;) {
    const page = await api.call<{ items: Event[]; next?: string }>('sessions.timeline', { id: sessionId, after, limit: 5000 });
    items.push(...page.items);
    if (!page.next) return items;
    after = page.next;
  }
}

async function replayText(windows: { windowId: string }[]): Promise<{ text: string; events: number }> {
  let text = '';
  let events = 0;
  for (const w of windows) {
    let afterSeq: number | undefined;
    for (;;) {
      const page = await api.call<{ events: unknown[]; nextSeq?: number }>('sessions.replay', {
        id: sessionId,
        windowId: w.windowId,
        afterSeq,
      });
      text += JSON.stringify(page.events);
      events += page.events.length;
      if (page.nextSeq === undefined || page.nextSeq === null) break;
      afterSeq = page.nextSeq;
    }
  }
  return { text, events };
}

test('every scenario is recorded, masked and readable', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto(`${SITE}/?key=${key}&host=${encodeURIComponent(OT)}&scenario=all&bodies=true`);
  await page.waitForFunction(() => (window as unknown as { OpenTrail?: { sessionId(): string } }).OpenTrail?.sessionId());
  sessionId = await page.evaluate(() => (window as unknown as { OpenTrail: { sessionId(): string } }).OpenTrail.sessionId());
  await interact(page);
  await page.waitForFunction(() => (window as unknown as { __trail: { finished: boolean } }).__trail.finished, null, { timeout: 30_000 });
  await page.waitForTimeout(2500);
  await page.goto(`${SITE}/page2`);
  await page.waitForFunction(() => (window as unknown as { OpenTrail?: { sessionId(): string } }).OpenTrail?.sessionId());
  expect(await page.evaluate(() => (window as unknown as { OpenTrail: { sessionId(): string } }).OpenTrail.sessionId())).toBe(sessionId);
  await page.click('#back');
  await page.waitForURL(`${SITE}/app/orders`);
  await page.waitForTimeout(3000);
  await page.goto('about:blank');
  expect(errors.sort()).toEqual(['scenario rejection', 'scenario thrown'].sort());

  let detail: Json & { pages: { url: string }[]; windows: { windowId: string }[] } = { pages: [], windows: [] };
  await expect
    .poll(
      async () => {
        detail = await api.call('sessions.get', { id: sessionId });
        return detail.pages.some((p) => p.url.endsWith('/page2'));
      },
      { timeout: 20_000 },
    )
    .toBe(true);

  expect(detail.userRef).toBe('e2e-user-1');
  expect(detail.release).toBe('testsite-1');
  expect(detail.browser).toMatch(/^Chrome/);
  expect(detail.windows).toHaveLength(1);
  const urls = detail.pages.map((p) => new URL(p.url).pathname);
  expect(urls).toEqual(expect.arrayContaining(['/', '/app/orders', '/app/orders/42', '/page2']));

  const events = await timeline();
  const of = (kind: string) => events.filter((e) => e.kind === kind).map((e) => e.data);

  const clicks = of('interaction').filter((i) => i.kind === 'click');
  expect(clicks).toContainEqual(
    expect.objectContaining({ role: 'button', name: 'Pay Now', testid: 'pay-button', component: 'CheckoutForm' }),
  );
  expect(clicks).toContainEqual(expect.objectContaining({ name: 'Show Details' }));
  expect(of('interaction')).toContainEqual(expect.objectContaining({ kind: 'dblclick', name: 'Double Click Me' }));
  expect(of('interaction')).toContainEqual(expect.objectContaining({ kind: 'contextmenu', testid: 'context-area' }));
  const inputs = of('interaction').filter((i) => i.kind === 'input');
  expect(inputs).toContainEqual(expect.objectContaining({ name: 'Personal Number', inputLength: 13 }));
  expect(inputs).toContainEqual(expect.objectContaining({ name: 'Card Number' }));

  const consoleRows = of('console');
  expect(consoleRows.map((c) => c.level)).toEqual(expect.arrayContaining(['log', 'info', 'warn', 'error', 'debug']));
  expect(consoleRows).toContainEqual(expect.objectContaining({ level: 'error', message: 'scenario error for ******-****' }));

  expect(of('error')).toContainEqual(expect.objectContaining({ kind: 'error', message: 'scenario thrown' }));
  expect(of('error')).toContainEqual(expect.objectContaining({ kind: 'unhandledrejection', message: 'scenario rejection' }));

  const network = of('network');
  const status = (path: string) => network.find((n) => String(n.url).includes(path))?.status;
  expect(status('/api/ok?via=xhr')).toBe(200);
  expect(status('/api/notfound')).toBe(404);
  expect(status('/api/error')).toBe(500);
  expect(status('127.0.0.1:9/unreachable')).toBe(0);
  expect(network.find((n) => String(n.url).includes('/api/slow'))?.durationMs as number).toBeGreaterThanOrEqual(700);
  expect(network.some((n) => n.initiator === 'xhr' && n.method === 'POST')).toBe(true);
  expect(network.map((n) => n.url)).toContain(`${SITE}/api/ok?token=****&page=2`);
  expect(network.some((n) => String(n.url).includes('/i/v1/batch'))).toBe(false);

  const echo = network.find((n) => n.initiator === 'fetch' && String(n.url).endsWith('/api/echo'));
  const bodies = await api.call<{ requestBody?: string; responseBody?: string }>('sessions.request_body', { id: echo?.id });
  expect(bodies.requestBody).toBe('{"pnr":"********-****","item":"book"}');
  expect(bodies.responseBody).toContain('********-****');

  expect(of('custom')).toContainEqual(expect.objectContaining({ name: 'scenario_custom', props: { step: 2, plan: 'pro' } }));
  expect(of('vital').map((v) => v.name)).toEqual(expect.arrayContaining(['TTFB', 'FCP']));

  const all = JSON.stringify(events);
  for (const secret of ['supersecret', 'scenario-secret-token', 'Bearer', '19811218', '4111 1111', 'hunter2']) {
    expect(all, `${secret} is not in the timeline`).not.toContain(secret);
  }

  const replay = await replayText(detail.windows);
  expect(replay.events).toBeGreaterThan(50);
  for (const secret of [
    'Ada Lovelace',
    'hunter2',
    '19811218',
    '4111 1111',
    'Secret note',
    'Blocked content',
    'supersecret',
    'javascript:',
    'onclick',
  ]) {
    expect(replay.text, `${secret} is not in the replay`).not.toContain(secret);
  }
  expect(replay.text).toContain('visible search');
  expect(replay.text).toContain('Inside the frame');
  expect(replay.text).toContain('Row 1999');
  expect(replay.text).not.toContain('"source":9');

  const list = await api.call<{ items: { id: string; bytes: number }[] }>('sessions.list', { projectId, hasErrors: true });
  const summary = list.items.find((s) => s.id === sessionId);
  expect(summary).toBeDefined();
  console.log(`session ${sessionId}: ${replay.events} events, ${summary?.bytes} bytes stored`);
});
