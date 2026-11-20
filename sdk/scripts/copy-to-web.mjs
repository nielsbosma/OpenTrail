// The server serves the script-tag build at /sdk.js from the web app's files.
import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const target = join(root, '..', 'web', 'public');
mkdirSync(target, { recursive: true });
copyFileSync(join(root, 'dist', 'sdk.js'), join(target, 'sdk.js'));
