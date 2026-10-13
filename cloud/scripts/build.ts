// Uses an explicitly supplied, already installed tool cache. Never runs Wrangler or reads its auth.
import { createRequire } from 'node:module';
import { mkdirSync } from 'node:fs';
import { resolve } from 'node:path';
const root = process.argv[2];
if (!root) throw new Error('Provide an existing tool project containing wrangler/esbuild; no installation is performed.');
const req = createRequire(resolve(root, 'package.json'));
const cached = createRequire(req.resolve('wrangler/package.json'));
const { build } = cached('esbuild');
mkdirSync('dist/cloud', { recursive: true });
await build({ entryPoints: ['cloud/src/worker.ts'], bundle: true, format: 'esm', target: 'es2022', external: ['cloudflare:workers'], outfile: 'dist/cloud/worker.js', sourcemap: false, minify: false });
console.log('Built dist/cloud/worker.js; no Cloudflare API was called.');
