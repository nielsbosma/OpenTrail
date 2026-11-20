// A site to record: static scenario pages and a few API endpoints that answer 200, 404, 500,
// slowly, or echo. No dependencies, so the container is just Node.
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(fileURLToPath(new URL('.', import.meta.url)), 'site');
const port = Number(process.env.PORT ?? 8092);
const types = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' };

const json = (res, status, body) => {
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(JSON.stringify(body));
};

const api = {
  '/api/ok': (_req, res) => json(res, 200, { ok: true }),
  '/api/notfound': (_req, res) => json(res, 404, { error: 'not found' }),
  '/api/error': (_req, res) => json(res, 500, { error: 'it broke' }),
  '/api/slow': (_req, res, url) =>
    setTimeout(() => json(res, 200, { slow: true }), Math.min(Number(url.searchParams.get('ms') ?? 1000), 10_000)),
  '/api/auth': (req, res) => json(res, req.headers.authorization ? 200 : 401, { authorized: !!req.headers.authorization }),
  '/api/echo': (req, res) => {
    let body = '';
    req.on('data', (chunk) => {
      body += chunk;
      if (body.length > 100_000) req.destroy();
    });
    req.on('end', () => json(res, 200, { echo: body }));
  },
};

const pages = { '/': 'index.html', '/page2': 'page2.html', '/frame': 'frame.html' };

createServer(async (req, res) => {
  const url = new URL(req.url ?? '/', 'http://localhost');
  const handler = api[url.pathname];
  if (handler) return handler(req, res, url);
  const file = pages[url.pathname] ?? (url.pathname.startsWith('/app/') ? 'index.html' : normalize(url.pathname).replace(/^[/\\]+/, ''));
  if (file.includes('..')) return json(res, 400, { error: 'bad path' });
  try {
    const body = await readFile(join(root, file));
    res.writeHead(200, { 'content-type': types[extname(file)] ?? 'application/octet-stream', 'cache-control': 'no-store' });
    res.end(body);
  } catch {
    json(res, 404, { error: 'not found' });
  }
}).listen(port, () => console.log(`test site on http://localhost:${port}`));
