// Personal credential authority. No model traffic, request bodies or tokens are logged.
export type Provider = 'codex' | 'claude';
export type Bundle = { provider: Provider; credential: any; identity: any };
export type Identity = { owner: string; display: string; plan: string; expires_at: number; refresh_expires_at: number | null };
type Entry = { id: string; label: string; bundle: Bundle; identity: Identity; version: number; state: string; source: string; operation: string; digest: string; last_refresh: number | null; error: string | null; retry_at: number; automatic: boolean; replacement?: Entry };
type Device = { id: string; label: string; secret_hash: string; revoked: boolean; last_seen: number };
type Pair = { id: string; code: string; label: string; proof: string; expires: number; approved: boolean; device?: Device; token?: string };
export type State = { schema: 1; accounts: Entry[]; devices: Device[]; pairs: Pair[] };
export interface Store { load(): Promise<State | null>; save(state: State): Promise<void>; }
export interface Issuer { verify(bundle: Bundle): Promise<void>; refresh(bundle: Bundle): Promise<Bundle>; usage(bundle: Bundle): Promise<any>; }
export class Fault extends Error { status: number; constructor(code: string, status = 409) { super(code); this.status = status; } }
export const epoch = () => Math.floor(Date.now() / 1000);
export const random = () => crypto.randomUUID().replaceAll('-', '') + crypto.randomUUID().replaceAll('-', '');
export async function hash(value: string): Promise<string> { return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value))), n => n.toString(16).padStart(2, '0')).join(''); }
export function text(value: unknown, maximum = 200): string { if (typeof value !== 'string' || !value.trim() || value.length > maximum || /[\x00-\x1f\x7f]/.test(value)) throw new Fault('invalid_input', 400); return value; }
function claims(token: string): any { try { return JSON.parse(atob(token.split('.')[1].replaceAll('-', '+').replaceAll('_', '/'))); } catch { throw new Fault('invalid_credential', 400); } }
function finite(value: unknown): number { if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) throw new Fault('invalid_expiry', 400); return value; }
function mask(email: string): string { const [name, domain] = email.split('@'); return domain ? `${name.slice(0, 1)}***@${domain}` : '未提供邮箱'; }
export function identity(bundle: Bundle): Identity {
  if (bundle.provider === 'codex') {
    const c = bundle.credential;
    if (c.OPENAI_API_KEY || (c.auth_mode && c.auth_mode !== 'chatgpt')) throw new Fault('subscription_required', 400);
    const id = claims(text(c.tokens?.id_token, 32768));
    const a = claims(text(c.tokens?.access_token, 32768));
    text(c.tokens?.refresh_token, 32768);
    const auth = a['https://api.openai.com/auth']; const idAuth = id['https://api.openai.com/auth'];
    const user = text(auth?.chatgpt_user_id || auth?.user_id); const workspace = text(auth?.chatgpt_account_id);
    if (workspace !== c.tokens.account_id || (idAuth?.chatgpt_user_id || idAuth?.user_id) !== user || idAuth?.chatgpt_account_id !== workspace) throw new Fault('identity_mismatch');
    return { owner: `codex:${user}:${workspace}`, display: mask(id.email || ''), plan: idAuth.chatgpt_plan_type || '未知', expires_at: finite(a.exp), refresh_expires_at: null };
  }
  if (bundle.provider !== 'claude') throw new Fault('invalid_provider', 400);
  const c = bundle.credential; text(c.accessToken, 32768); text(c.refreshToken, 32768);
  if (!Array.isArray(c.scopes) || !c.scopes.includes('user:inference')) throw new Fault('inference_scope_required', 400);
  const i = bundle.identity;
  return { owner: `claude:${text(i.accountUuid)}:${text(i.organizationUuid)}`, display: mask(text(i.emailAddress)), plan: c.subscriptionType || '未知', expires_at: Math.floor(finite(c.expiresAt) / 1000), refresh_expires_at: c.refreshTokenExpiresAt ? Math.floor(finite(c.refreshTokenExpiresAt) / 1000) : null };
}
function usageCredential(bundle: Bundle): Bundle {
  return bundle.provider === 'codex'
    ? { provider: 'codex', credential: { tokens: { access_token: bundle.credential.tokens.access_token, account_id: bundle.credential.tokens.account_id } }, identity: null }
    : { provider: 'claude', credential: { accessToken: bundle.credential.accessToken }, identity: null };
}
function safe(a: Entry): any { return { id: a.id, provider: a.bundle.provider, label: a.label, identity: a.identity.display, plan: a.identity.plan, expires_at: a.identity.expires_at, refresh_expires_at: a.identity.refresh_expires_at, version: a.version, state: a.state, automatic: a.automatic, last_refresh: a.last_refresh, error: a.error, retry_at: a.retry_at }; }
export class Authority {
  store: Store; issuer: Issuer; clock: () => number; tail: Promise<unknown> = Promise.resolve();
  constructor(store: Store, issuer: Issuer, clock = epoch) { this.store = store; this.issuer = issuer; this.clock = clock; }
  async run(action: string, body: any, bearer: string | null, admin = false): Promise<any> {
    const task = this.tail.then(() => this.operation(action, body, bearer, admin));
    this.tail = task.catch(() => {}); return task;
  }
  private async operation(action: string, body: any, bearer: string | null, admin: boolean): Promise<any> {
    const s = await this.store.load() || { schema: 1, accounts: [], devices: [], pairs: [] } as State;
    if (s.schema !== 1) throw new Fault('schema_unavailable', 503);
    const now = this.clock();
    if (!body || typeof body !== 'object' || Array.isArray(body)) throw new Fault('invalid_input', 400);
    s.pairs = s.pairs.filter(p => p.expires > now);
    if (action === 'pair.begin') {
      if (s.pairs.length >= 24) throw new Fault('pair_limit', 429);
      const proof = text(body.proof, 128); if (!/^[a-f0-9]{64}$/.test(proof)) throw new Fault('invalid_proof', 400);
      const p: Pair = { id: crypto.randomUUID(), code: random().slice(0, 12).toUpperCase(), label: text(body.label, 80), proof: await hash(proof), expires: now + 600, approved: false };
      s.pairs.push(p); await this.store.save(s); return { id: p.id, code: p.code, expires_at: p.expires };
    }
    if (action === 'pair.poll') {
      const p = s.pairs.find(p => p.id === body.id && p.proof === bearer);
      if (!p) throw new Fault('pair_expired', 401);
      return p.approved ? { state: 'paired', device_id: p.device!.id, device_token: p.token } : { state: 'pending' };
    }
    if (action === 'admin.pairs' && admin) return { pairs: s.pairs.filter(p => !p.approved).map(p => ({ id: p.id, code: p.code, label: p.label, expires_at: p.expires })) };
    if (action === 'admin.approve' && admin) {
      const p = s.pairs.find(p => p.id === body.id && p.code === body.code);
      if (!p) throw new Fault('pair_expired');
      if (!p.approved) { const token = random(); p.device = { id: crypto.randomUUID(), label: p.label, secret_hash: await hash(token), revoked: false, last_seen: now }; p.token = token; p.approved = true; s.devices.push(p.device); await this.store.save(s); }
      return { approved: true };
    }
    if (action === 'admin.devices' && admin) return { devices: s.devices.map(d => ({ id: d.id, label: d.label, revoked: d.revoked, last_seen: d.last_seen })) };
    if (action === 'admin.revoke' && admin) { const d = s.devices.find(d => d.id === body.id); if (!d) throw new Fault('device_missing'); d.revoked = true; s.pairs = s.pairs.filter(p => p.device?.id !== d.id); await this.store.save(s); return { revoked: true }; }
    if (action === 'maintenance' && admin) {
      for (const a of s.accounts) if (a.automatic) {
        try { if (a.state === 'refreshing') { a.state = 'uncertain'; a.error = 'refresh_outcome_unknown'; await this.store.save(s); } else if (a.state === 'verification_pending') await this.verifyRotated(s, a); else if (a.state === 'uncertain' || (a.state === 'ready' && a.identity.expires_at < now + 900)) await this.renew(s, a); }
        catch { /* State is persisted; one account cannot starve maintenance for the others. */ }
      }
      return { ok: true };
    }
    const device = s.devices.find(d => !d.revoked && d.secret_hash === bearer);
    if (!device) throw new Fault('device_unauthorized', 401);
    if (action === 'accounts') return { accounts: s.accounts.map(safe), devices: s.devices.filter(d => !d.revoked).map(d => ({ id: d.id, label: d.label, last_seen: d.last_seen })) };
    if (action === 'import') {
      const operation = text(body.operation, 80); const bundle = body.bundle as Bundle; const id = identity(bundle);
      const digest = await hash(JSON.stringify(bundle));
      const old = s.accounts.find(a => a.operation === operation && a.source === device.id);
      if (old) { if (old.digest !== digest) throw new Fault('import_changed'); return safe(old); }
      const replacement = body.replace_id ? s.accounts.find(a => a.id === body.replace_id) : undefined;
      if (body.replace_id && (!replacement || replacement.identity.owner !== id.owner)) throw new Fault('identity_mismatch');
      if (replacement && ['awaiting_handoff', 'refreshing'].includes(replacement.state)) throw new Fault('handoff_in_progress');
      if (s.accounts.some(a => a.identity.owner === id.owner && a !== replacement)) throw new Fault('account_already_registered');
      if (!replacement && s.accounts.length >= 100) throw new Fault('account_limit');
      // No refresh on import. Verify the current access through a read-only issuer endpoint.
      if (id.expires_at <= now + 60) throw new Fault('source_access_expired');
      await this.issuer.verify(bundle);
      const a: Entry = { id: crypto.randomUUID(), label: text(body.label, 100), bundle, identity: id, version: 1, state: 'awaiting_handoff', source: device.id, operation, digest, last_refresh: null, error: null, retry_at: 0, automatic: false };
      if (replacement) { a.id = replacement.id; a.version = replacement.version + 1; a.label = replacement.label; a.replacement = structuredClone(replacement); s.accounts = s.accounts.filter(x => x !== replacement); }
      s.accounts.push(a); device.last_seen = now; await this.store.save(s); return safe(a);
    }
    if (action === 'import.status') { const a = s.accounts.find(a => a.operation === body.operation && a.source === device.id); return { account: a ? safe(a) : null }; }
    if (action === 'import.abort') { const a = s.accounts.find(a => a.operation === body.operation && a.source === device.id); if (a && a.state !== 'awaiting_handoff') throw new Fault('handoff_already_committed'); s.accounts = s.accounts.filter(x => x !== a); if (a?.replacement) s.accounts.push(a.replacement); await this.store.save(s); return { aborted: true }; }
    const a = s.accounts.find(a => a.id === body.id); if (!a) throw new Fault('account_missing', 404);
    if (action === 'import.commit') {
      if (a.source !== device.id || a.operation !== body.operation) throw new Fault('source_device_required', 403);
      if (a.state === 'awaiting_handoff') { a.state = 'ready'; a.automatic = true; delete a.replacement; await this.store.save(s); }
      return safe(a);
    }
    if (action === 'update') { if (body.label !== undefined) a.label = text(body.label, 100); if (typeof body.automatic === 'boolean') a.automatic = body.automatic && a.state !== 'awaiting_handoff'; await this.store.save(s); return safe(a); }
    if (action === 'remove') { if (a.state === 'refreshing' || a.state === 'uncertain') throw new Fault('recovery_required'); s.accounts = s.accounts.filter(x => x !== a); await this.store.save(s); return { removed: true }; }
    if (!['lease', 'check', 'usage'].includes(action)) throw new Fault('unknown_action', 400);
    if (a.state === 'awaiting_handoff') throw new Fault('handoff_required');
    if (action === 'usage') {
      if (a.state !== 'ready' || a.identity.expires_at <= now + 60) throw new Fault('usage_credential_unavailable');
      return await this.issuer.usage(usageCredential(a.bundle));
    }
    if (a.state === 'refreshing') { a.state = 'uncertain'; a.error = 'refresh_outcome_unknown'; await this.store.save(s); }
    if (a.state === 'verification_pending') await this.verifyRotated(s, a);
    if (a.state === 'uncertain' && a.automatic) await this.renew(s, a);
    if (a.state !== 'ready') throw new Fault(a.error || 'reauthorization_required');
    if (action !== 'usage' && (a.identity.expires_at < now + 600 || (body.rejected_version !== undefined && body.rejected_version === a.version))) {
      if (!a.automatic) throw new Fault('maintenance_disabled');
      await this.renew(s, a);
    }
    if (a.state !== 'ready' || a.identity.expires_at <= now + 60) throw new Fault(a.error || 'credential_expired');
    device.last_seen = now; await this.store.save(s);
    if (action === 'check') { await this.issuer.verify(a.bundle); return safe(a); }
    const c = a.bundle.credential;
    // Native projection never includes refresh. ID token is only for the Codex native format.
    const projection = a.bundle.provider === 'codex' ? { provider: 'codex', credential: { auth_mode: 'chatgptAuthTokens', tokens: { access_token: c.tokens.access_token, id_token: c.tokens.id_token, account_id: c.tokens.account_id }, last_refresh: c.last_refresh || null }, identity: null } : { provider: 'claude', credential: Object.fromEntries(['accessToken', 'scopes', 'expiresAt', 'subscriptionType', 'rateLimitTier'].filter(k => c[k] !== undefined).map(k => [k,c[k]])), identity: a.bundle.identity };
    return { id: a.id, provider: a.bundle.provider, label: a.label, owner: a.identity.owner, version: a.version, expires_at: a.identity.expires_at, access_token: a.bundle.provider === 'codex' ? c.tokens.access_token : c.accessToken, projection };
  }
  private async renew(s: State, a: Entry): Promise<void> {
    if (a.retry_at > this.clock()) throw new Fault('retry_later', 503);
    a.state = 'refreshing'; a.error = null; await this.store.save(s);
    let next: Bundle;
    try { next = await this.issuer.refresh(a.bundle); const id = identity(next); if (id.owner !== a.identity.owner) throw new Fault('identity_mismatch'); a.bundle = next; a.identity = id; }
    catch (e) {
      // 429: nothing was exchanged. A refusal or a foreign identity only reauthorization can fix.
      // Anything else may have been received, but no newer credential exists: the same one is
      // granted again after retry_at, which can only succeed or be refused.
      const code = e instanceof Fault ? e.message : '';
      const terminal = code === 'refresh_rejected' || code === 'identity_mismatch';
      a.state = code === 'issuer_rate_limited' ? 'ready' : terminal ? 'reauthorization_required' : 'uncertain';
      a.error = code === 'issuer_rate_limited' || terminal ? code : 'refresh_outcome_unknown';
      a.retry_at = this.clock() + 60; await this.store.save(s); throw new Fault(a.error, terminal ? 409 : 503);
    }
    a.version++; a.state = 'verification_pending'; a.error = null; a.last_refresh = this.clock(); a.retry_at = 0;
    await this.store.save(s); // A failed commit leaves the durable refresh intent, reconciled like any lost reply.
    await this.verifyRotated(s, a);
  }
  private async verifyRotated(s: State, a: Entry): Promise<void> {
    if (a.retry_at > this.clock()) throw new Fault('verification_pending', 503);
    try { await this.issuer.verify(a.bundle); }
    catch (e) {
      // The rotated grant stays; the one it replaced is never sent again. The issuer refusing the access it
      // just issued, or a foreign identity, only reauthorization can fix. Anything else is verified again later.
      const code = e instanceof Fault ? e.message : '';
      const refused = code === 'issuer_unauthorized' || code === 'identity_mismatch';
      a.state = refused ? 'reauthorization_required' : 'verification_pending'; a.error = refused ? code : 'verification_pending';
      a.retry_at = this.clock() + 60; await this.store.save(s); throw new Fault(a.error, refused ? 409 : 503);
    }
    a.state = 'ready'; a.error = null; await this.store.save(s);
  }
}
