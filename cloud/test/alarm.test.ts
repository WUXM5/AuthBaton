import assert from 'node:assert/strict';
import fs from 'node:fs';
import { stripTypeScriptTypes } from 'node:module';
import { test } from 'node:test';
import { Fault, hash } from '../src/core.ts';

// Exercise the actual Vault methods. Only the Cloudflare runtime, Authority and
// clock are replaced. A DO has one alarm timestamp; setAlarm replaces that time.
// This fixture does not evaluate account/issuer code or access real storage/network.
const file = new URL('../src/worker.ts', import.meta.url);
const original = fs.readFileSync(file, 'utf8');
const compiled = stripTypeScriptTypes(original, { mode: 'strip' })
  .replace(/^import[^\n]+;\s*$/gm, '')
  .replace('export class Vault', 'class Vault')
  .replace('export default {', 'const worker = {');
assert(compiled.includes('class Vault extends DurableObject'));

function fixture(negativeControl = false) {
  let now = 1800000000000;
  const start = now;
  let nextAlarm: number | null = null;
  let ticks = 0;
  const writes: number[] = [];
  const actions: { action: string; at: number }[] = [];
  const storage = {
    async getAlarm(): Promise<number | null> { return nextAlarm; },
    async setAlarm(time: number | Date): Promise<void> { nextAlarm = Number(time); writes.push(nextAlarm); },
  };
  class DurableObject {
    ctx: any;
    env: any;
    constructor(ctx: any, env: any) { this.ctx = ctx; this.env = env; }
  }
  class Authority {
    async run(action: string): Promise<any> { actions.push({ action, at: now }); return { ok: true }; }
  }
  const forbidden = (): never => { throw new Error('Unexpected non-synthetic boundary'); };
  const { Vault } = new Function('DurableObject', 'Authority', 'Fault', 'hash', 'OfficialIssuer', 'owner', 'adminHTML', 'Date',
    `${compiled}\nreturn {Vault};`)(DurableObject, Authority, Fault, hash, class {}, forbidden, '', { now: () => now });
  const vault = new Vault({ storage }, {});
  async function request(): Promise<void> {
    const response = await vault.fetch(new Request('https://synthetic.internal/', {
      method: 'POST', body: JSON.stringify({ action: 'accounts', body: {}, bearer: 'SYNTHETIC', admin: false }),
    }));
    assert.equal(response.status, 200);
    if (negativeControl) await storage.setAlarm(now + 240000);
  }
  async function advance(seconds: number): Promise<void> {
    const target = start + seconds * 1000;
    while (nextAlarm !== null && nextAlarm <= target) {
      now = nextAlarm; nextAlarm = null;
      ticks++;
      assert(ticks <= 20, 'Alarm fixture must make bounded forward progress');
      await vault.alarm();
    }
    now = target;
  }
  return { request, advance, snapshot: () => ({ ticks, writes: [...writes], actions: [...actions], nextAlarmSeconds: nextAlarm === null ? null : (nextAlarm - start) / 1000 }) };
}

async function traffic(negativeControl = false) {
  const f = fixture(negativeControl);
  for (let seconds = 0; seconds <= 600; seconds += 30) {
    await f.advance(seconds);
    await f.request();
  }
  return f.snapshot();
}

test('actual Vault alarm continues recurring without foreground requests', async () => {
  const f = fixture(); await f.request(); await f.advance(600);
  const result = f.snapshot();
  assert.equal(result.ticks, 2);
  assert.equal(result.actions.filter(a => a.action === 'maintenance').length, 2);
  assert.equal(result.nextAlarmSeconds, 720);
});

test('21 successful requests across ten minutes do not postpone automatic maintenance', async t => {
  const result = await traffic();
  t.diagnostic(JSON.stringify({ syntheticRequests: 21, simulatedSeconds: 600, maintenanceRuns: result.actions.filter(a => a.action === 'maintenance').length, alarmWrites: result.writes.length, nextAlarmSeconds: result.nextAlarmSeconds, realNetworkRequests: 0 }));
  assert.equal(result.ticks, 2, 'Requests must preserve an existing earlier alarm rather than continually replace it with now + 240 seconds');
  assert.equal(result.actions.filter(a => a.action === 'maintenance').length, 2);
  assert.equal(result.nextAlarmSeconds, 720);
});

test('negative control: reinstating unconditional setAlarm after every request starves maintenance', async () => {
  const result = await traffic(true);
  assert.equal(result.ticks, 0);
  assert.equal(result.actions.filter(a => a.action === 'maintenance').length, 0);
  assert.equal(result.nextAlarmSeconds, 840);
});
