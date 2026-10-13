import { DurableObject } from 'cloudflare:workers';
import { Authority, Fault, hash, type State, type Store } from './core.ts';
import { OfficialIssuer } from './issuer.ts';
import { owner } from './access.ts';
import { adminHTML } from './admin.ts';
interface Env { VAULT: any; VAULT_KEY: string; ACCESS_TEAM: string; ACCESS_AUD: string; OWNER_EMAIL: string; }
const headers = { 'Cache-Control': 'no-store', 'Content-Type': 'application/json', 'X-Content-Type-Options': 'nosniff' };
const json = (value: any, status = 200) => new Response(JSON.stringify(value), { status, headers });
const b64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));
const bytes = (s: string) => Uint8Array.from(atob(s), c => c.charCodeAt(0));
export class Vault extends DurableObject<Env> {
  authority: Authority;
  constructor(ctx: any, env: Env) {
    super(ctx, env);
    const key = async () => { let raw: Uint8Array; try { raw = bytes(env.VAULT_KEY); } catch { throw new Fault('deployment_not_configured', 503); } if (raw.length !== 32) throw new Fault('deployment_not_configured', 503); return crypto.subtle.importKey('raw', raw, 'AES-GCM', false, ['encrypt', 'decrypt']); };
    const store: Store = {
      load: async () => { const v = await ctx.storage.get('vault-v1') as any; if (!v) return null; const plain = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: bytes(v.nonce), additionalData: new TextEncoder().encode('authmanager/personal/v1') }, await key(), bytes(v.data)); return JSON.parse(new TextDecoder().decode(plain)); },
      save: async (state: State) => { const nonce = crypto.getRandomValues(new Uint8Array(12)); const data = await crypto.subtle.encrypt({ name: 'AES-GCM', iv: nonce, additionalData: new TextEncoder().encode('authmanager/personal/v1') }, await key(), new TextEncoder().encode(JSON.stringify(state))); await ctx.storage.put('vault-v1', { key_version: 1, nonce: b64(nonce), data: b64(new Uint8Array(data)) }); }
    };
    this.authority = new Authority(store, new OfficialIssuer());
  }
  async fetch(request: Request): Promise<Response> {
    try { const { action, body, bearer, admin, source } = await request.json() as any; const result = await this.authority.run(action, body, bearer, admin, source); await this.schedule(); return json(result); }
    catch (e) { return json({ error: e instanceof Fault ? e.message : 'service_unavailable' }, e instanceof Fault ? e.status : 503); }
  }
  private async schedule(): Promise<void> { const existing = await this.ctx.storage.getAlarm(); const next = Date.now() + 240000; if (existing === null || existing === undefined || existing > next) await this.ctx.storage.setAlarm(next); }
  async alarm(): Promise<void> { try { await this.authority.run('maintenance', {}, null, true); } finally { await this.schedule(); } }
}
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    try {
      const url = new URL(request.url);
      if (url.pathname === '/health') return json({ service: 'authmanager-personal-cloud', protocol: 1 });
      if (!env.VAULT_KEY || !env.ACCESS_TEAM || !env.ACCESS_AUD || !env.OWNER_EMAIL) throw new Fault('deployment_not_configured', 503);
      const admin = url.pathname.startsWith('/admin');
      if (admin) await owner(request, env);
      if (url.pathname === '/admin' && request.method === 'GET') return new Response(adminHTML, { headers: { ...headers, 'Content-Type': 'text/html; charset=utf-8', 'Content-Security-Policy': "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'" } });
      const prefix = admin ? '/admin/api/' : '/v1/';
      if (!url.pathname.startsWith(prefix) || request.method !== 'POST') throw new Fault('not_found', 404);
      if (request.headers.get('Origin') && request.headers.get('Origin') !== url.origin) throw new Fault('origin_forbidden', 403);
      if (!request.headers.get('Content-Type')?.startsWith('application/json')) throw new Fault('json_required', 415);
      const raw = await request.text(); if (raw.length > 131072) throw new Fault('request_too_large', 413);
      const action = url.pathname.slice(prefix.length); if (action.startsWith('admin.') !== admin || action === 'maintenance') throw new Fault('forbidden', 403);
      const token = request.headers.get('Authorization')?.replace(/^Bearer /, '') || '';
      const stub = env.VAULT.get(env.VAULT.idFromName('personal-v1'));
      // Cloudflare sets CF-Connecting-IP to the client address; a client cannot supply its own.
      const source = request.headers.get('CF-Connecting-IP') || '';
      return await stub.fetch(new Request('https://internal/', { method: 'POST', body: JSON.stringify({ action, body: JSON.parse(raw), bearer: token ? await hash(token) : null, admin, source }) }));
    } catch (e) { return json({ error: e instanceof Fault ? e.message : 'invalid_request' }, e instanceof Fault ? e.status : 400); }
  }
};
