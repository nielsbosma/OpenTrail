// Loads the OpenTrail SDK from the server named in the URL and runs the scenarios that need no
// user: console, errors, network, navigation, DOM changes, canvas and custom events. Clicks and
// typing come from a person or from Playwright, since only real input looks like real input.
(() => {
  const params = new URLSearchParams(location.search);
  const saved = JSON.parse(sessionStorage.getItem('testsite') ?? '{}');
  const config = {
    key: params.get('key') ?? saved.key,
    host: params.get('host') ?? saved.host,
    bodies: (params.get('bodies') ?? String(saved.bodies)) === 'true',
  };
  const status = document.getElementById('status');
  window.__trail = { done: [], finished: false };
  if (!config.key || !config.host) return;
  sessionStorage.setItem('testsite', JSON.stringify(config));

  const sdk = document.createElement('script');
  sdk.src = `${config.host}/sdk.js`;
  sdk.dataset.key = config.key;
  sdk.dataset.host = config.host;
  sdk.dataset.release = 'testsite-1';
  sdk.dataset.captureBodies = String(config.bodies);
  sdk.onload = () => {
    if (status) status.textContent = `Recording session ${window.OpenTrail?.sessionId() ?? '(not sampled)'}`;
    const scenario = params.get('scenario');
    if (scenario) void run(scenario === 'all' ? Object.keys(scenarios) : scenario.split(','));
  };
  sdk.onerror = () => {
    if (status) status.textContent = `Could not load ${sdk.src}`;
  };
  document.head.appendChild(sdk);

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  const quietly = (p) =>
    p.then(
      () => {},
      () => {},
    );

  const scenarios = {
    async console() {
      console.log('scenario log', { cart: 3 });
      console.info('scenario info');
      console.warn('scenario warn');
      console.error('scenario error for 811218-9876');
      console.debug('scenario debug');
    },
    async errors() {
      setTimeout(() => {
        throw new Error('scenario thrown');
      }, 0);
      Promise.reject(new Error('scenario rejection'));
      await wait(50);
    },
    async network() {
      await quietly(fetch('/api/ok'));
      await quietly(fetch('/api/notfound'));
      await quietly(fetch('/api/error'));
      await quietly(fetch('/api/slow?ms=800'));
      await quietly(fetch('http://127.0.0.1:9/unreachable'));
      await quietly(fetch('/api/auth', { headers: { Authorization: 'Bearer scenario-secret-token' } }));
      await quietly(fetch('/api/ok?token=supersecret&page=2'));
      await quietly(
        fetch('/api/echo', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ pnr: '19811218-9876', item: 'book' }),
        }),
      );
      await new Promise((resolve) => {
        const xhr = new XMLHttpRequest();
        xhr.open('GET', '/api/ok?via=xhr');
        xhr.onloadend = resolve;
        xhr.send();
      });
      await new Promise((resolve) => {
        const xhr = new XMLHttpRequest();
        xhr.open('POST', '/api/echo');
        xhr.setRequestHeader('content-type', 'application/json');
        xhr.onloadend = resolve;
        xhr.send(JSON.stringify({ via: 'xhr' }));
      });
    },
    async spa() {
      history.pushState({}, '', '/app/orders');
      document.title = 'Orders';
      await wait(120);
      history.pushState({}, '', '/app/orders/42');
      document.title = 'Order 42';
      await wait(120);
      history.back();
      await wait(200);
    },
    async mutations() {
      const rows = document.getElementById('rows');
      const fragment = document.createDocumentFragment();
      for (let i = 0; i < 2000; i++) {
        const li = document.createElement('li');
        li.textContent = `Row ${i}`;
        fragment.appendChild(li);
      }
      rows?.appendChild(fragment);
      await wait(50);
      for (let i = 0; i < 50; i++) rows?.children[i]?.setAttribute('data-touched', String(i));
    },
    async canvas() {
      const ctx = document.getElementById('canvas')?.getContext('2d');
      if (!ctx) return;
      ctx.fillStyle = '#0b62d6';
      ctx.fillRect(10, 10, 100, 40);
    },
    async custom() {
      window.OpenTrail?.track('scenario_custom', { step: 2, plan: 'pro' });
      window.OpenTrail?.identify('e2e-user-1');
    },
  };

  async function run(names) {
    for (const name of names) {
      try {
        await scenarios[name]?.();
      } catch (error) {
        console.error('scenario failed', name, error);
      }
      window.__trail.done.push(name);
    }
    window.__trail.finished = true;
    if (status) status.textContent += ' · scenarios done';
  }
  window.__trail.run = run;
})();
