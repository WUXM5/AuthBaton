import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Authority, Fault, hash, identity, type Bundle, type Issuer, type State, type Store } from '../src/core.ts';
import { OfficialIssuer } from '../src/issuer.ts';
import { owner } from '../src/access.ts';

// All values are synthetic. No real transport, file, environment or credential store.
class Memory implements Store {
  value: State | null = null;
  saves = 0;
  fault: ((value: State) => 'before' | 'after' | undefined) | undefined;
  async load(): Promise<State | null> { return structuredClone(this.value); }
  async save(value: State): Promise<void> {
    this.saves++;
    const failure = this.fault?.(value);
    if (failure === 'before') throw new Fault('synthetic_store_failure', 503);
    this.value = structuredClone(value);
    if (failure === 'after') throw new Fault('synthetic_store_ack_failure', 503);
  }
}
function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>(r => { resolve = r; });
  return { promise, resolve };
}
function bundle(provider: 'claude' | 'codex' = 'claude', generation = 1, user = 'synthetic-user'): Bundle {
  const expiry = Math.floor(Date.now() / 1000) + 3600;
  if (provider === 'claude') return {
    provider,
    credential: { accessToken: `SYNTHETIC-access-${generation}`, refreshToken: `SYNTHETIC-refresh-${generation}`, expiresAt: expiry * 1000,
      refreshTokenExpiresAt: (expiry + 86400) * 1000, scopes: ['user:inference', 'user:profile'], subscriptionType: 'pro' },
    identity: { accountUuid: user, organizationUuid: 'synthetic-org', emailAddress: 'synthetic@example.invalid' },
  };
  const claims = { email: 'synthetic@example.invalid', exp: expiry, sub: user,
    'https://api.openai.com/auth': { chatgpt_user_id: user, chatgpt_account_id: 'synthetic-workspace', chatgpt_plan_type: 'pro' } };
  const jwt = `${Buffer.from('{"alg":"none"}').toString('base64url')}.${Buffer.from(JSON.stringify(claims)).toString('base64url')}.SYNTHETIC-${generation}`;
  return { provider, credential: { auth_mode: 'chatgpt', tokens: { access_token: jwt, id_token: jwt, refresh_token: `SYNTHETIC-refresh-${generation}`, account_id: 'synthetic-workspace' } }, identity: {} };
}
class SyntheticIssuer implements Issuer {
  refreshes = 0;
  verifies = 0;
  usageCalls = 0;
  refreshInputs: Bundle[] = [];
  beforeVerify: (() => Promise<void>) | undefined;
  beforeRefresh: (() => Promise<void>) | undefined;
  refreshedUser: string | null = null;
  async verify(_bundle: Bundle): Promise<void> { this.verifies++; await this.beforeVerify?.(); }
  async refresh(value: Bundle): Promise<Bundle> {
    this.refreshes++; this.refreshInputs.push(structuredClone(value)); await this.beforeRefresh?.();
    return bundle(value.provider, this.refreshes + 1, this.refreshedUser ?? (value.provider === 'claude' ? value.identity.accountUuid : 'synthetic-user'));
  }
  async usage(_bundle: Bundle): Promise<any> { this.usageCalls++; return { windows: [] }; }
}
async function pair(authority: Authority, suffix = 'a') {
  const proof = suffix.repeat(64);
  const pending = await authority.run('pair.begin', { proof, label: `synthetic-${suffix}` }, null);
  const proofHash = await hash(proof);
  await authority.run('admin.approve', { id: pending.id, code: pending.code }, null, true);
  const response = await authority.run('pair.poll', { id: pending.id }, proofHash);
  return { id: response.device_id as string, bearer: await hash(response.device_token), pending, proofHash };
}
async function setup(provider: 'claude' | 'codex' = 'claude', issuer: Issuer = new SyntheticIssuer()) {
  const store = new Memory(); let time = Math.floor(Date.now() / 1000);
  const clock = () => time;
  const authority = new Authority(store, issuer, clock);
  const device = await pair(authority);
  const imported = await authority.run('import', { operation: 'synthetic-import', label: 'Synthetic account', bundle: bundle(provider) }, device.bearer);
  const id = imported.id as string;
  const commit = () => authority.run('import.commit', { id, operation: 'synthetic-import' }, device.bearer);
  return { authority, store, issuer, device, id, commit, clock, advance: (seconds: number) => { time += seconds; } };
}
const code = (expected: string) => (error: unknown): boolean => error instanceof Fault && error.message === expected;

test('pairing requires both owner approval and the original private proof', async () => {
  const authority = new Authority(new Memory(), new SyntheticIssuer());
  const proof = 'a'.repeat(64); const p = await authority.run('pair.begin', { proof, label: 'Synthetic' }, null);
  assert.deepEqual(await authority.run('pair.poll', { id: p.id }, await hash(proof)), { state: 'pending' });
  await assert.rejects(authority.run('admin.approve', { id: p.id, code: p.code, admin: true }, null), code('device_unauthorized'));
  await assert.rejects(authority.run('admin.approve', { id: p.id, code: 'WRONG' }, null, true), code('pair_expired'));
  await authority.run('admin.approve', { id: p.id, code: p.code }, null, true);
  await assert.rejects(authority.run('pair.poll', { id: p.id }, await hash(p.code)), code('pair_expired'));
  await assert.rejects(authority.run('pair.poll', { id: p.id }, await hash('b'.repeat(64))), code('pair_expired'));
  assert.equal((await authority.run('pair.poll', { id: p.id }, await hash(proof))).state, 'paired');
});

test('expired pairing cannot release an approved device credential', async () => {
  let time = Math.floor(Date.now() / 1000); const authority = new Authority(new Memory(), new SyntheticIssuer(), () => time);
  const d = await pair(authority); time += 601;
  await assert.rejects(authority.run('pair.poll', { id: d.pending.id }, d.proofHash), code('pair_expired'));
});

test('revocation rejects the device and removes its pending credential retrieval', async () => {
  const f = await setup(); await f.commit();
  await f.authority.run('admin.revoke', { id: f.device.id }, null, true);
  await assert.rejects(f.authority.run('accounts', {}, f.device.bearer), code('device_unauthorized'));
  await assert.rejects(f.authority.run('lease', { id: f.id }, f.device.bearer), code('device_unauthorized'));
  await assert.rejects(f.authority.run('pair.poll', { id: f.device.pending.id }, f.device.proofHash), code('pair_expired'));
});

test('only importing device can commit handoff; import alone cannot refresh or lease', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); const other = await pair(f.authority, 'b');
  await assert.rejects(f.authority.run('import.commit', { id: f.id, operation: 'synthetic-import' }, other.bearer), code('source_device_required'));
  await assert.rejects(f.authority.run('import.commit', { id: f.id, operation: 'wrong' }, f.device.bearer), code('source_device_required'));
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('handoff_required'));
  await f.authority.run('maintenance', {}, null, true);
  assert.equal(issuer.refreshes, 0);
  assert.equal((await f.commit()).state, 'ready');
});

test('import abort is source-bound and idempotent, and cannot abort an already committed handoff', async () => {
  const f = await setup(); const other = await pair(f.authority, 'b');
  await f.authority.run('import.abort', { operation: 'synthetic-import' }, other.bearer);
  assert.equal(f.store.value!.accounts.length, 1);
  await f.authority.run('import.abort', { operation: 'synthetic-import' }, f.device.bearer);
  assert.equal(f.store.value!.accounts.length, 0);
  assert.deepEqual(await f.authority.run('import.abort', { operation: 'synthetic-import' }, f.device.bearer), { aborted: true });
  const g = await setup(); await g.commit();
  await assert.rejects(g.authority.run('import.abort', { operation: 'synthetic-import' }, g.device.bearer), code('handoff_already_committed'));
  assert.equal(g.store.value!.accounts.length, 1);
});

test('idempotent import preserves source identity and rejects a changed payload or duplicate identity', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer);
  const original = structuredClone(f.store.value!.accounts[0].bundle);
  const repeat = await f.authority.run('import', { operation: 'synthetic-import', label: 'other', bundle: original }, f.device.bearer);
  assert.equal(repeat.id, f.id); assert.equal(issuer.verifies, 1);
  await assert.rejects(f.authority.run('import', { operation: 'synthetic-import', label: 'other', bundle: bundle('claude', 2) }, f.device.bearer), code('import_changed'));
  await assert.rejects(f.authority.run('import', { operation: 'new-operation', label: 'other', bundle: original }, f.device.bearer), code('account_already_registered'));
});

for (const provider of ['claude', 'codex'] as const) {
  test(`${provider}: replacement preserves account ID, commits a new version, and never refreshes the superseded grant`, async () => {
    const issuer = new SyntheticIssuer(); const f = await setup(provider, issuer); await f.commit();
    const original = structuredClone(f.store.value!.accounts[0]);
    const fresh = bundle(provider, 100);
    const request = { operation: 'synthetic-replacement', replace_id: f.id, label: 'Ignored replacement label', bundle: fresh };
    const imported = await f.authority.run('import', request, f.device.bearer);
    assert.equal(imported.id, f.id); assert.equal(imported.label, original.label); assert.equal(imported.version, original.version + 1);
    assert.equal(imported.state, 'awaiting_handoff'); assert.equal(imported.automatic, false);
    assert.deepEqual(f.store.value!.accounts[0].replacement, original);
    assert.equal((await f.authority.run('import', request, f.device.bearer)).version, 2);
    assert.equal(f.store.value!.accounts.length, 1); assert.equal(issuer.refreshes, 0); assert.equal(issuer.verifies, 2);
    await assert.rejects(f.authority.run('lease', { id: f.id }, f.device.bearer), code('handoff_required'));
    await assert.rejects(f.authority.run('import', { ...request, operation: 'overlapping-replacement' }, f.device.bearer), code('handoff_in_progress'));
    await f.authority.run('maintenance', {}, null, true); assert.equal(issuer.refreshes, 0);
    await f.authority.run('import.commit', { id: f.id, operation: request.operation }, f.device.bearer);
    assert.equal('replacement' in f.store.value!.accounts[0], false);
    assert.deepEqual(f.store.value!.accounts[0].bundle, fresh);
    await f.authority.run('lease', { id: f.id, rejected_version: 2 }, f.device.bearer);
    assert.equal(issuer.refreshes, 1); assert.deepEqual(issuer.refreshInputs, [fresh]);
  });
}

test('replacement abort restores the exact original record, including maintenance preference and credential generation', async () => {
  const f = await setup(); await f.commit();
  await f.authority.run('update', { id: f.id, automatic: false, label: 'Keep original' }, f.device.bearer);
  const original = structuredClone(f.store.value!.accounts[0]);
  await f.authority.run('import', { operation: 'replacement-to-abort', replace_id: f.id, label: 'New', bundle: bundle('claude', 100) }, f.device.bearer);
  await f.authority.run('import.abort', { operation: 'replacement-to-abort' }, f.device.bearer);
  assert.deepEqual(f.store.value!.accounts, [original]);
  await f.authority.run('import.abort', { operation: 'replacement-to-abort' }, f.device.bearer);
  assert.deepEqual(f.store.value!.accounts, [original]);
  assert.deepEqual(await f.authority.run('import.status', { operation: 'replacement-to-abort' }, f.device.bearer), { account: null });
});

test('replacement rejects another identity or provider before verifying and keeps the original untouched', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  const original = structuredClone(f.store.value!.accounts);
  for (const fresh of [bundle('claude', 100, 'other-owner'), bundle('codex', 100)]) {
    await assert.rejects(f.authority.run('import', { operation: 'invalid-replacement', replace_id: f.id, label: 'New', bundle: fresh }, f.device.bearer), code('identity_mismatch'));
    assert.deepEqual(f.store.value!.accounts, original);
  }
  await assert.rejects(f.authority.run('import', { operation: 'missing-target', replace_id: 'missing', label: 'New', bundle: bundle() }, f.device.bearer), code('identity_mismatch'));
  assert.equal(issuer.verifies, 1); assert.equal(issuer.refreshes, 0);
});

test('replacement handoff belongs to its new importing device; the former source cannot commit or abort it', async () => {
  const f = await setup(); await f.commit(); const nextSource = await pair(f.authority, 'b');
  await f.authority.run('import', { operation: 'other-device-replacement', replace_id: f.id, label: 'New', bundle: bundle('claude', 100) }, nextSource.bearer);
  await assert.rejects(f.authority.run('import.commit', { id: f.id, operation: 'other-device-replacement' }, f.device.bearer), code('source_device_required'));
  await f.authority.run('import.abort', { operation: 'other-device-replacement' }, f.device.bearer);
  assert.equal(f.store.value!.accounts[0].state, 'awaiting_handoff');
  await f.authority.run('import.commit', { id: f.id, operation: 'other-device-replacement' }, nextSource.bearer);
  assert.equal(f.store.value!.accounts[0].source, nextSource.id); assert.equal(f.store.value!.accounts[0].state, 'ready');
});

for (const failure of ['before', 'after'] as const) {
  test(`replacement import persistence failure ${failure} commit can be reconciled without duplicating an account`, async () => {
    const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
    const original = structuredClone(f.store.value!.accounts[0]);
    const request = { operation: 'recover-import', replace_id: f.id, label: 'New', bundle: bundle('claude', 100) };
    f.store.fault = value => value.accounts[0].operation === request.operation ? failure : undefined;
    await assert.rejects(f.authority.run('import', request, f.device.bearer));
    if (failure === 'before') assert.deepEqual(f.store.value!.accounts[0], original);
    else assert.equal(f.store.value!.accounts[0].state, 'awaiting_handoff');
    f.store.fault = undefined;
    const restarted = new Authority(f.store, issuer, f.clock);
    const status = await restarted.run('import.status', { operation: request.operation }, f.device.bearer);
    assert.equal(status.account?.id ?? null, failure === 'after' ? f.id : null);
    assert.equal((await restarted.run('import', request, f.device.bearer)).version, 2);
    assert.equal(f.store.value!.accounts.length, 1);
    await restarted.run('import.abort', { operation: request.operation }, f.device.bearer);
    assert.deepEqual(f.store.value!.accounts[0], original); assert.equal(issuer.refreshes, 0);
  });
  test(`replacement handoff commit acknowledgement ${failure} persistence reconciles the correct generation`, async () => {
    const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
    const operation = 'commit-replacement';
    await f.authority.run('import', { operation, replace_id: f.id, label: 'New', bundle: bundle('claude', 100) }, f.device.bearer);
    f.store.fault = value => value.accounts[0].operation === operation && value.accounts[0].state === 'ready' ? failure : undefined;
    await assert.rejects(f.authority.run('import.commit', { id: f.id, operation }, f.device.bearer));
    f.store.fault = undefined;
    const restarted = new Authority(f.store, issuer, f.clock);
    const status = await restarted.run('import.status', { operation }, f.device.bearer);
    assert.equal(status.account.state, failure === 'before' ? 'awaiting_handoff' : 'ready');
    assert.equal('replacement' in f.store.value!.accounts[0], failure === 'before');
    if (failure === 'after') await assert.rejects(restarted.run('import.abort', { operation }, f.device.bearer), code('handoff_already_committed'));
    assert.equal((await restarted.run('import.commit', { id: f.id, operation }, f.device.bearer)).version, 2);
    assert.equal('replacement' in f.store.value!.accounts[0], false);
    assert.equal(f.store.value!.accounts[0].bundle.credential.refreshToken, 'SYNTHETIC-refresh-100');
    assert.equal(issuer.refreshes, 0);
  });
}

test('same-identity reauthorization recovers an uncertain account without touching its old grant again', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  issuer.beforeRefresh = async () => { throw new Fault('issuer_outcome_unknown', 503); };
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('refresh_outcome_unknown'));
  assert.equal(f.store.value!.accounts[0].state, 'uncertain');
  await f.authority.run('import', { operation: 'reauthorized', replace_id: f.id, label: 'New', bundle: bundle('claude', 100) }, f.device.bearer);
  await f.authority.run('import.commit', { id: f.id, operation: 'reauthorized' }, f.device.bearer);
  const result = await f.authority.run('lease', { id: f.id }, f.device.bearer);
  assert.equal(result.version, 2); assert.equal(result.access_token, 'SYNTHETIC-access-100');
  assert.equal(issuer.refreshes, 1); assert.equal('replacement' in f.store.value!.accounts[0], false);
});

test('reauthorization at the account limit does not require an additional account slot', async () => {
  const f = await setup(); await f.commit(); const original = f.store.value!.accounts[0];
  for (let n = 0; n < 99; n++) {
    const value = bundle('claude', 1, `other-synthetic-${n}`);
    f.store.value!.accounts.push({ ...structuredClone(original), id: `other-id-${n}`, operation: `other-operation-${n}`, bundle: value, identity: identity(value) });
  }
  const replaced = await f.authority.run('import', { operation: 'full-directory-replacement', replace_id: f.id, label: 'New', bundle: bundle('claude', 100) }, f.device.bearer);
  assert.equal(replaced.id, f.id); assert.equal(f.store.value!.accounts.length, 100);
});

test('usage near expiry ignores rejected_version and disabled maintenance, without changing authentication state', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  await f.authority.run('update', { id: f.id, automatic: false }, f.device.bearer);
  f.advance(3300);
  const before = structuredClone(f.store.value!.accounts); const saves = f.store.saves;
  assert.deepEqual(await f.authority.run('usage', { id: f.id, rejected_version: 1 }, f.device.bearer), { windows: [] });
  assert.equal(issuer.usageCalls, 1); assert.equal(issuer.refreshes, 0); assert.equal(issuer.verifies, 1);
  assert.deepEqual(f.store.value!.accounts, before);
  assert.equal(f.store.saves, saves);
});

test('usage with expired access rejects without refreshing, verifying, or altering the account', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit(); f.advance(3601);
  const before = structuredClone(f.store.value!.accounts); const saves = f.store.saves;
  await assert.rejects(f.authority.run('usage', { id: f.id }, f.device.bearer), code('usage_credential_unavailable'));
  assert.equal(issuer.usageCalls, 0); assert.equal(issuer.refreshes, 0); assert.equal(issuer.verifies, 1);
  assert.deepEqual(f.store.value!.accounts, before);
  assert.equal(f.store.saves, saves);
});

test('usage cannot recover verification_pending or mutate a pending rotation', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  issuer.beforeVerify = async () => { throw new Fault('issuer_network_error', 503); };
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('verification_pending'));
  issuer.beforeVerify = undefined;
  const before = structuredClone(f.store.value!.accounts); const verifies = issuer.verifies; const saves = f.store.saves;
  await assert.rejects(f.authority.run('usage', { id: f.id }, f.device.bearer), code('usage_credential_unavailable'));
  assert.equal(issuer.verifies, verifies); assert.equal(issuer.refreshes, 1);
  assert.deepEqual(f.store.value!.accounts, before);
  assert.equal(issuer.usageCalls, 0); assert.equal(f.store.saves, saves);
});

test('usage cannot turn a saved refreshing intent into uncertain or start its recovery', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  f.store.fault = value => value.accounts[0].state === 'verification_pending' ? 'before' : undefined;
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer)); f.store.fault = undefined;
  const before = structuredClone(f.store.value!.accounts); const verifies = issuer.verifies; const saves = f.store.saves;
  await assert.rejects(new Authority(f.store, issuer, f.clock).run('usage', { id: f.id }, f.device.bearer), code('usage_credential_unavailable'));
  assert.equal(issuer.verifies, verifies); assert.equal(issuer.refreshes, 1);
  assert.deepEqual(f.store.value!.accounts, before);
  assert.equal(issuer.usageCalls, 0); assert.equal(f.store.saves, saves);
});

test('20 concurrent rejections of one version exchange once and never deliver refresh tokens', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  const results = await Promise.all(Array.from({ length: 20 }, () => f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer)));
  assert.equal(issuer.refreshes, 1); assert.deepEqual([...new Set(results.map(r => r.version))], [2]);
  assert.equal(results.some(r => JSON.stringify(r).includes('SYNTHETIC-refresh')), false);
  assert.equal(results.some(r => 'refreshToken' in r.projection.credential), false);
});

test('a revocation queued first cannot be bypassed by a later lease', async () => {
  const f = await setup(); await f.commit();
  const revoke = f.authority.run('admin.revoke', { id: f.device.id }, null, true);
  const lease = f.authority.run('lease', { id: f.id }, f.device.bearer);
  await revoke; await assert.rejects(lease, code('device_unauthorized'));
});

test('awaited issuer work serializes revocation; after acknowledgement no later delivery is permitted', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  const entered = deferred(), release = deferred();
  issuer.beforeRefresh = async () => { entered.resolve(); await release.promise; };
  const events: string[] = [];
  const inFlight = f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer).then(result => { events.push('lease'); return result; });
  await entered.promise;
  const revoke = f.authority.run('admin.revoke', { id: f.device.id }, null, true).then(result => { events.push('revoke'); return result; });
  release.resolve(); await inFlight; await revoke;
  assert.deepEqual(events, ['lease', 'revoke']);
  await assert.rejects(f.authority.run('lease', { id: f.id }, f.device.bearer), code('device_unauthorized'));
  // Revocation is not retroactive: the already-authorized preceding request finished.
});

test('failure before saving refresh intent never reaches the issuer', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  f.store.fault = value => value.accounts[0].state === 'refreshing' ? 'before' : undefined;
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('synthetic_store_failure'));
  assert.equal(issuer.refreshes, 0); assert.equal(f.store.value!.accounts[0].state, 'ready');
});

test('an ambiguous intent-save acknowledgement is reconciled by granting the unchanged credential', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  f.store.fault = value => value.accounts[0].state === 'refreshing' ? 'after' : undefined;
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer));
  f.store.fault = undefined;
  assert.equal(issuer.refreshes, 0);
  const restarted = new Authority(f.store, issuer, f.clock);
  assert.equal((await restarted.run('lease', { id: f.id }, f.device.bearer)).version, 2);
  assert.equal(issuer.refreshes, 1);
  assert.equal(issuer.refreshInputs[0].credential.refreshToken, 'SYNTHETIC-refresh-1');
});
test('a rotation lost before it was saved is reconciled by re-granting the credential that was sent', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  f.store.fault = value => value.accounts[0].state === 'verification_pending' ? 'before' : undefined;
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('synthetic_store_failure'));
  assert.equal(f.store.value!.accounts[0].state, 'refreshing');
  assert.equal(f.store.value!.accounts[0].version, 1);
  assert.equal(issuer.verifies, 1, 'Only initial import verified; the unsaved rotation must not reach verify');
  f.store.fault = undefined;
  const result = await new Authority(f.store, issuer, f.clock).run('lease', { id: f.id }, f.device.bearer);
  assert.equal(result.version, 2); assert.equal(f.store.value!.accounts[0].state, 'ready');
  assert.equal(issuer.refreshes, 2);
  assert.deepEqual(issuer.refreshInputs.map(b => b.credential.refreshToken), ['SYNTHETIC-refresh-1', 'SYNTHETIC-refresh-1']);
});
test('verification_pending persisted with a lost acknowledgement resumes verification of the new version', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  f.store.fault = value => value.accounts[0].state === 'verification_pending' ? 'after' : undefined;
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer));
  assert.equal(f.store.value!.accounts[0].state, 'verification_pending');
  assert.equal(f.store.value!.accounts[0].bundle.credential.refreshToken, 'SYNTHETIC-refresh-2');
  assert.equal(issuer.verifies, 1);
  f.store.fault = undefined;
  const result = await new Authority(f.store, issuer, f.clock).run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer);
  assert.equal(result.version, 2); assert.equal(issuer.refreshes, 1);
  assert.equal(issuer.verifies, 2); assert.equal(f.store.value!.accounts[0].state, 'ready');
});

test('new grant is durable before verify awaits; a failed verify survives restart and only re-verifies', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  const entered = deferred(), release = deferred();
  issuer.beforeVerify = async () => { entered.resolve(); await release.promise; throw new Fault('issuer_network_error', 503); };
  const operation = f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer);
  await entered.promise;
  assert.equal(f.store.value!.accounts[0].state, 'verification_pending');
  assert.equal(f.store.value!.accounts[0].version, 2);
  assert.equal(f.store.value!.accounts[0].bundle.credential.refreshToken, 'SYNTHETIC-refresh-2');
  release.resolve(); await assert.rejects(operation, code('verification_pending'));
  issuer.beforeVerify = undefined;
  const result = await new Authority(f.store, issuer, f.clock).run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer);
  assert.equal(result.version, 2); assert.equal(issuer.refreshes, 1); assert.equal(issuer.verifies, 3);
});

test('a failed error-state save after verify failure still retains the rotated credential for recovery', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  issuer.beforeVerify = async () => { throw new Fault('issuer_network_error', 503); };
  f.store.fault = value => value.accounts[0].error === 'verification_pending' ? 'before' : undefined;
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('synthetic_store_failure'));
  assert.equal(f.store.value!.accounts[0].state, 'verification_pending');
  assert.equal(f.store.value!.accounts[0].bundle.credential.refreshToken, 'SYNTHETIC-refresh-2');
  issuer.beforeVerify = undefined; f.store.fault = undefined;
  assert.equal((await new Authority(f.store, issuer, f.clock).run('lease', { id: f.id }, f.device.bearer)).version, 2);
  assert.equal(issuer.refreshes, 1);
});

for (const failure of ['before', 'after'] as const) {
  test(`verified-ready save failure ${failure} persistence recovers without another grant`, async () => {
    const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
    f.store.fault = value => value.accounts[0].state === 'ready' && value.accounts[0].version === 2 ? failure : undefined;
    await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer));
    assert.equal(f.store.value!.accounts[0].state, failure === 'before' ? 'verification_pending' : 'ready');
    f.store.fault = undefined;
    const result = await new Authority(f.store, issuer, f.clock).run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer);
    assert.equal(result.version, 2); assert.equal(issuer.refreshes, 1);
    assert.equal(issuer.verifies, failure === 'before' ? 3 : 2);
  });
}

test('alarm maintenance recovers verification_pending without another grant', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  issuer.beforeVerify = async () => { throw new Fault('issuer_network_error', 503); };
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('verification_pending'));
  issuer.beforeVerify = undefined;
  await new Authority(f.store, issuer, f.clock).run('maintenance', {}, null, true);
  assert.equal(f.store.value!.accounts[0].state, 'ready'); assert.equal(f.store.value!.accounts[0].version, 2);
  assert.equal(issuer.refreshes, 1); assert.equal(issuer.verifies, 3);
});

test('one account failing pending verification cannot starve another due account', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  issuer.beforeVerify = async () => { throw new Fault('issuer_network_error', 503); };
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('verification_pending'));
  issuer.beforeVerify = undefined;
  const secondBundle = bundle('claude', 1, 'second-synthetic-user'); secondBundle.credential.expiresAt = (f.clock() + 100) * 1000;
  const second = await f.authority.run('import', { operation: 'second-operation', label: 'Second', bundle: secondBundle }, f.device.bearer);
  await f.authority.run('import.commit', { id: second.id, operation: 'second-operation' }, f.device.bearer);
  issuer.verify = async value => { issuer.verifies++; if (value.identity.accountUuid === 'synthetic-user') throw new Fault('issuer_network_error', 503); };
  await new Authority(f.store, issuer, f.clock).run('maintenance', {}, null, true);
  assert.equal(f.store.value!.accounts[0].state, 'verification_pending');
  assert.equal(f.store.value!.accounts[1].state, 'ready'); assert.equal(f.store.value!.accounts[1].version, 2);
  assert.equal(issuer.refreshes, 2);
});

test('issuer identity changes cannot replace the bound account and require reauthorization', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  const oldOwner = identity(f.store.value!.accounts[0].bundle).owner; issuer.refreshedUser = 'different-synthetic-user';
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('identity_mismatch'));
  assert.equal(identity(f.store.value!.accounts[0].bundle).owner, oldOwner);
  assert.equal(f.store.value!.accounts[0].state, 'reauthorization_required');
  assert.equal(f.store.value!.accounts[0].version, 1); assert.equal(issuer.refreshes, 1);
  f.advance(61); await f.authority.run('maintenance', {}, null, true);
  await assert.rejects(f.authority.run('lease', { id: f.id }, f.device.bearer), code('identity_mismatch'));
  assert.equal(issuer.refreshes, 1);
});

test('a refused grant requires reauthorization, is never retried, and reauthorization replaces it', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  issuer.beforeRefresh = async () => { throw new Fault('refresh_rejected'); };
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('refresh_rejected'));
  assert.equal(f.store.value!.accounts[0].state, 'reauthorization_required');
  f.advance(61); await f.authority.run('maintenance', {}, null, true);
  await assert.rejects(f.authority.run('lease', { id: f.id }, f.device.bearer), code('refresh_rejected'));
  assert.equal(issuer.refreshes, 1);
  await f.authority.run('import', { operation: 'reauthorized', replace_id: f.id, label: 'New', bundle: bundle('claude', 100) }, f.device.bearer);
  await f.authority.run('import.commit', { id: f.id, operation: 'reauthorized' }, f.device.bearer);
  assert.equal((await f.authority.run('lease', { id: f.id }, f.device.bearer)).access_token, 'SYNTHETIC-access-100');
});

test('an unanswered grant is retried with the same credential once retry_at passes', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  issuer.beforeRefresh = async () => { if (issuer.refreshes === 1) throw new Fault('issuer_outcome_unknown', 503); };
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('refresh_outcome_unknown'));
  assert.equal(f.store.value!.accounts[0].state, 'uncertain');
  await f.authority.run('maintenance', {}, null, true);
  await assert.rejects(f.authority.run('lease', { id: f.id }, f.device.bearer), code('retry_later'));
  assert.equal(issuer.refreshes, 1);
  f.advance(61); await f.authority.run('maintenance', {}, null, true);
  assert.equal(f.store.value!.accounts[0].state, 'ready'); assert.equal(f.store.value!.accounts[0].version, 2);
  assert.deepEqual(issuer.refreshInputs.map(b => b.credential.refreshToken), ['SYNTHETIC-refresh-1', 'SYNTHETIC-refresh-1']);
});

for (const [provider, status, error, expected] of [
  ['claude', 400, { error: 'invalid_grant' }, 'refresh_rejected'],
  ['claude', 400, { error: { type: 'invalid_grant' } }, 'refresh_rejected'],
  ['codex', 401, { error: { code: 'refresh_token_reused' } }, 'refresh_rejected'],
  ['claude', 400, { error: 'invalid_request' }, 'issuer_unavailable'],
  ['claude', 500, { error: 'invalid_grant' }, 'issuer_unavailable'],
] as const) {
  test(`${provider}: token endpoint ${status} ${JSON.stringify(error)} is classified as ${expected}`, async () => {
    const originalFetch = globalThis.fetch;
    globalThis.fetch = (async () => Response.json(error, { status })) as typeof fetch;
    try { await assert.rejects(new OfficialIssuer().refresh(bundle(provider)), code(expected)); }
    finally { globalThis.fetch = originalFetch; }
  });
}
for (const provider of ['claude', 'codex'] as const) {
  test(`${provider}: a verification 429 AFTER a successful grant must never make the OLD grant retryable`, async t => {
    const originalFetch = globalThis.fetch;
    let exchanging = false; const posted: string[] = [];
    let durable: Memory | undefined;
    const verifySawRotatedSaved: boolean[] = [];
    globalThis.fetch = (async (url: any, options: any = {}) => {
      const address = String(url);
      if (options.method === 'POST') {
        posted.push(JSON.parse(options.body).refresh_token);
        exchanging = true;
        if (provider === 'claude') return Response.json({ access_token: 'SYNTHETIC-rotated-access', refresh_token: 'SYNTHETIC-rotated-refresh', expires_in: 3600, scope: 'user:inference user:profile' });
        const c = bundle('codex', 2).credential;
        return Response.json({ access_token: c.tokens.access_token, refresh_token: c.tokens.refresh_token, id_token: c.tokens.id_token });
      }
      if (exchanging) {
        const c = durable?.value?.accounts[0]?.bundle.credential;
        verifySawRotatedSaved.push(provider === 'claude' ? c?.refreshToken === 'SYNTHETIC-rotated-refresh' : c?.tokens?.refresh_token === 'SYNTHETIC-refresh-2');
        return new Response('{}', { status: 429 });
      }
      if (address.includes('/api/oauth/profile')) return Response.json({ account: { uuid: 'synthetic-user' }, organization: { uuid: 'synthetic-org' } });
      return Response.json({ rate_limit: {} });
    }) as typeof fetch;
    try {
      const f = await setup(provider, new OfficialIssuer()); durable = f.store; await f.commit();
      await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer));
      f.advance(61);
      await f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer).catch(() => {});
      t.diagnostic(JSON.stringify({ provider, syntheticTokenPosts: posted.length, oldGrantReplays: Math.max(0, posted.filter(token => token === 'SYNTHETIC-refresh-1').length - 1), verifySawRotatedSaved, realNetworkRequests: 0 }));
      assert.equal(posted.length, 1, 'A token exchange already succeeded; a later verify 429 must not replay its old refresh token');
      assert.deepEqual(verifySawRotatedSaved, [true, true], 'Both initial verification and verification retry must use already committed new credentials');
      assert.equal(f.store.value!.accounts[0].state, 'verification_pending');
      assert.equal(f.store.value!.accounts[0].version, 2);
    } finally { globalThis.fetch = originalFetch; }
  });
}

test('explicit 429 at the token endpoint can retry after backoff; no credential exchange occurred', async () => {
  const issuer = new SyntheticIssuer(); let calls = 0;
  issuer.refresh = async value => { calls++; if (calls === 1) throw new Fault('issuer_rate_limited', 503); return bundle(value.provider, 2); };
  const f = await setup('claude', issuer); await f.commit();
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('issuer_rate_limited'));
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('retry_later'));
  assert.equal(calls, 1); f.advance(61);
  assert.equal((await f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer)).version, 2);
  assert.equal(calls, 2);
});

test('owner Access JWT requires a finite expiry, in addition to a valid issuer signature', async () => {
  const key = await crypto.subtle.generateKey({ name: 'RSASSA-PKCS1-v1_5', modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: 'SHA-256' }, true, ['sign', 'verify']);
  const jwk = { ...await crypto.subtle.exportKey('jwk', key.publicKey), kid: 'synthetic-key' };
  const env = { ACCESS_TEAM: 'synthetic-team', ACCESS_AUD: 'synthetic-audience', OWNER_EMAIL: 'synthetic@example.invalid' };
  const fetcher = (async () => Response.json({ keys: [jwk] })) as typeof fetch;
  async function signed(exp: unknown, include = true, extra: Record<string, unknown> = {}): Promise<Request> {
    const head = Buffer.from(JSON.stringify({ alg: 'RS256', kid: jwk.kid })).toString('base64url');
    const payload = Buffer.from(JSON.stringify({ iss: 'https://synthetic-team.cloudflareaccess.com', aud: [env.ACCESS_AUD], email: env.OWNER_EMAIL, ...(include ? { exp } : {}), ...extra })).toString('base64url');
    const sig = await crypto.subtle.sign('RSASSA-PKCS1-v1_5', key.privateKey, new TextEncoder().encode(`${head}.${payload}`));
    return new Request('https://synthetic.invalid/admin', { headers: { 'Cf-Access-Jwt-Assertion': `${head}.${payload}.${Buffer.from(sig).toString('base64url')}` } });
  }
  await owner(await signed(Math.floor(Date.now() / 1000) + 600), env, fetcher);
  await assert.rejects(owner(await signed(undefined, false), env, fetcher), code('owner_unauthorized'));
  await assert.rejects(owner(await signed('not-a-time'), env, fetcher), code('owner_unauthorized'));
  const expiry = Math.floor(Date.now() / 1000) + 600;
  await assert.rejects(owner(await signed(expiry, true, { nbf: 'not-a-time' }), env, fetcher), code('owner_unauthorized'));
  await assert.rejects(owner(await signed(expiry, true, { nbf: null }), env, fetcher), code('owner_unauthorized'));
  await assert.rejects(owner(await signed(expiry, true, { nbf: expiry + 1 }), env, fetcher), code('owner_unauthorized'));
  await assert.rejects(owner(await signed(expiry, true, { email: 'different@example.invalid' }), env, fetcher), code('owner_unauthorized'));
  await assert.rejects(owner(await signed(expiry, true, { aud: ['different-audience'] }), env, fetcher), code('owner_unauthorized'));
});

test('Claude millisecond expiries with nonzero subsecond parts are projected as integer seconds', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  const value = bundle('claude', 100);
  const accessSeconds = f.clock() + 3600;
  const refreshSeconds = f.clock() + 86400;
  value.credential.expiresAt = accessSeconds * 1000 + 987;
  value.credential.refreshTokenExpiresAt = refreshSeconds * 1000 + 654;
  const metadata = identity(value);
  assert.equal(metadata.expires_at, accessSeconds); assert.equal(metadata.refresh_expires_at, refreshSeconds);
  const operation = 'fractional-ms-reauthorization';
  const imported = await f.authority.run('import', { operation, replace_id: f.id, label: 'New', bundle: value }, f.device.bearer);
  assert.equal(imported.expires_at, accessSeconds); assert.equal(imported.refresh_expires_at, refreshSeconds);
  await f.authority.run('import.commit', { id: f.id, operation }, f.device.bearer);
  const response = JSON.parse(JSON.stringify(await f.authority.run('lease', { id: f.id }, f.device.bearer)));
  assert.equal(response.expires_at, accessSeconds); assert(Number.isInteger(response.expires_at));
  assert.equal(response.projection.credential.expiresAt, value.credential.expiresAt, 'Native credential keeps its original millisecond timestamp');
  assert.equal(issuer.refreshes, 0);
});

test('alarm reconciles a persisted interrupted refresh by re-granting the credential that was sent', async () => {
  const issuer = new SyntheticIssuer(); const f = await setup('claude', issuer); await f.commit();
  f.store.fault = value => value.accounts[0].state === 'verification_pending' ? 'before' : undefined;
  await assert.rejects(f.authority.run('lease', { id: f.id, rejected_version: 1 }, f.device.bearer), code('synthetic_store_failure'));
  assert.equal(f.store.value!.accounts[0].state, 'refreshing'); assert.equal(issuer.refreshes, 1);
  const previousBundle = structuredClone(f.store.value!.accounts[0].bundle);
  f.store.fault = undefined;
  const restarted = new Authority(f.store, issuer, f.clock);
  await restarted.run('maintenance', {}, null, true);
  assert.equal(f.store.value!.accounts[0].state, 'uncertain');
  assert.equal(f.store.value!.accounts[0].error, 'refresh_outcome_unknown');
  assert.equal(f.store.value!.accounts[0].version, 1);
  assert.deepEqual(f.store.value!.accounts[0].bundle, previousBundle);
  await restarted.run('maintenance', {}, null, true);
  assert.equal(issuer.refreshes, 2); assert.deepEqual(issuer.refreshInputs[1], previousBundle);
  assert.equal(f.store.value!.accounts[0].state, 'ready'); assert.equal(f.store.value!.accounts[0].version, 2);
  assert.equal((await restarted.run('lease', { id: f.id }, f.device.bearer)).version, 2);
  assert.equal(issuer.refreshes, 2);
});
test('usage adapter never receives a refresh credential or native identity bundle', async () => {
  for (const provider of ['codex', 'claude'] as const) {
    const issuer = new SyntheticIssuer();
    let received: Bundle | undefined;
    issuer.usage = async value => { received = value; return { windows: [] }; };
    const h = await setup(provider, issuer); await h.commit();
    const saves = h.store.saves;
    await h.authority.run('usage', { id: h.id }, h.device.bearer);
    assert.ok(received);
    assert.equal(JSON.stringify(received).includes('SYNTHETIC-refresh'), false);
    assert.equal(received.identity, null);
    assert.equal(h.store.saves, saves);
    assert.equal(issuer.refreshes, 0);
  }
});
