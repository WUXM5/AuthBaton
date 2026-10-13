// Explicit personal-account deployment only. Never reads Wrangler state or .env.
import { readFile, stat, writeFile, mkdir } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve } from 'node:path';

const account = process.env.CLOUDFLARE_ACCOUNT_ID ?? '';
if (!/^[0-9a-f]{32}$/.test(account)) throw new Error('Set CLOUDFLARE_ACCOUNT_ID to the target Cloudflare account ID.');
const script = 'authmanager-personal';
const base = `https://api.cloudflare.com/client/v4/accounts/${account}/workers/scripts/${script}`;
const root = resolve(import.meta.dirname, '../..');
const source = await readFile(resolve(root, 'dist/cloud/worker.js'), 'utf8');
const metadata = {
  main_module: 'worker.js', compatibility_date: '2026-09-18',
  bindings: [{ type: 'durable_object_namespace', name: 'VAULT', class_name: 'Vault' }],
  migrations: { new_tag: 'v1', new_sqlite_classes: ['Vault'] },
  keep_bindings: ['plain_text', 'json', 'secret_text', 'secret_key'],
  observability: { enabled: false },
};
const plan = { account, script, endpoint: base, sha256: createHash('sha256').update(source).digest('hex'), metadata };
if (process.argv[2] !== '--apply') {
  console.log(JSON.stringify(plan, null, 2));
} else {
  const path = resolve(root, 'dist/cloud-bootstrap/personal-api-token');
  const mode = (await stat(path)).mode;
  if (mode & 0o077) throw new Error('Personal API token must be private (0600).');
  const token = (await readFile(path, 'utf8')).trim();
  if (!/^[A-Za-z0-9_-]{30,200}$/.test(token)) throw new Error('Invalid personal API token format.');
  const headers = { Authorization: `Bearer ${token}` };
  const before = await fetch(`${base}/settings`, { headers, redirect: 'error', signal: AbortSignal.timeout(20000) });
  if (!before.ok) throw new Error(`Preflight failed: HTTP ${before.status}; no deployment submitted.`);
  const settings = await before.json() as any;
  if (!settings.success || !Array.isArray(settings.result?.bindings)) throw new Error('Unrecognized settings; no deployment submitted.');
  if (settings.result.bindings.some((b: any) => !['plain_text', 'json', 'secret_text', 'secret_key'].includes(b.type))) {
    throw new Error('Worker already has resource bindings. Reconcile before a new migration.');
  }
  const form = new FormData();
  form.set('metadata', new Blob([JSON.stringify(metadata)], { type: 'application/json' }));
  form.set('worker.js', new Blob([source], { type: 'application/javascript+module' }), 'worker.js');
  const response = await fetch(base, { method: 'PUT', headers, body: form, redirect: 'error', signal: AbortSignal.timeout(60000) });
  const value = await response.json() as any;
  if (!response.ok || !value.success) {
    const codes = Array.isArray(value.errors) ? value.errors.map((e: any) => e.code).filter((n: any) => Number.isInteger(n)) : [];
    throw new Error(`Deployment unconfirmed: HTTP ${response.status}; codes ${codes.join(',')}. Inspect before retrying.`);
  }
  const report = { ...plan, deployed_at: new Date().toISOString(), version: value.result?.version_id ?? null, etag: value.result?.etag ?? null, migration_tag: value.result?.migration_tag ?? null };
  await mkdir(resolve(root, 'dist/cloud'), { recursive: true });
  await writeFile(resolve(root, 'dist/cloud/deployment-receipt.json'), JSON.stringify(report, null, 2));
  console.log(JSON.stringify({ deployed: true, account, script, version: report.version, migration_tag: report.migration_tag }));
}
