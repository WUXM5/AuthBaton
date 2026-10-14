import assert from 'node:assert/strict';
import fs from 'node:fs';
import { stripTypeScriptTypes } from 'node:module';
import { test } from 'node:test';
import { Fault, hash } from '../src/core.ts';

// The actual worker entry and Vault.fetch. Only the Cloudflare runtime and Authority are replaced;
// no storage, Access check, issuer or network is reached.
const compiled = stripTypeScriptTypes(fs.readFileSync(new URL('../src/worker.ts', import.meta.url), 'utf8'), { mode: 'strip' })
  .replace(/^import[^\n]+;\s*$/gm, '')
  .replace('export class Vault', 'class Vault')
  .replace('export default {', 'const worker = {');

test('the connecting address reaches the Authority, so pairing is limited and shown per network', async () => {
  const calls: unknown[][] = [];
  class DurableObject { ctx: any; env: any; constructor(ctx: any, env: any) { this.ctx = ctx; this.env = env; } }
  class Authority { async run(...args: unknown[]): Promise<any> { calls.push(args); return { id: 'synthetic' }; } }
  const forbidden = (): never => { throw new Error('Unexpected non-synthetic boundary'); };
  const { Vault, worker } = new Function('DurableObject', 'Authority', 'Fault', 'hash', 'OfficialIssuer', 'owner', 'adminHTML',
    `${compiled}\nreturn {Vault, worker};`)(DurableObject, Authority, Fault, hash, class {}, forbidden, '');
  const vault = new Vault({ storage: { getAlarm: async () => 1, setAlarm: forbidden } }, {});
  const env = { VAULT_KEY: 'synthetic', ACCESS_TEAM: 'synthetic', ACCESS_AUD: 'synthetic', OWNER_EMAIL: 'synthetic@example.invalid',
    VAULT: { idFromName: (name: string) => name, get: () => vault } };
  const body = { proof: 'a'.repeat(64), label: 'Synthetic Mac' };
  const response = await worker.fetch(new Request('https://synthetic.invalid/v1/pair.begin', {
    method: 'POST', headers: { 'Content-Type': 'application/json', 'CF-Connecting-IP': '203.0.113.7' }, body: JSON.stringify(body),
  }), env);
  assert.equal(response.status, 200);
  assert.deepEqual(calls, [['pair.begin', body, null, false, '203.0.113.7']]);
});
