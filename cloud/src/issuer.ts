import { Fault, identity, type Bundle, type Issuer } from './core.ts';

// The issuer refused this refresh token: RFC 6749 invalid_grant, or a Codex refresh_token_* code.
function rejected(content: string): boolean {
  let e: any; try { e = JSON.parse(content).error; } catch { return false; }
  return [e, e?.code, e?.type].some(c => c === 'invalid_grant' || (typeof c === 'string' && c.startsWith('refresh_token_')));
}
async function request(url: string, token?: string, body?: any, extra: Record<string, string> = {}): Promise<any> {
  let r: Response;
  try { r = await fetch(url, { method: body ? 'POST' : 'GET', redirect: 'manual', signal: AbortSignal.timeout(15000), headers: { accept: 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}), ...(body ? { 'content-type': 'application/json' } : {}), ...extra }, ...(body ? { body: JSON.stringify(body) } : {}) }); }
  catch { throw new Fault(body ? 'issuer_outcome_unknown' : 'issuer_network_error', 503); }
  if (!r.ok && body && (r.status === 400 || r.status === 401) && rejected(await r.text().catch(() => ''))) throw new Fault('refresh_rejected');
  if (!r.ok) throw new Fault(r.status === 429 ? 'issuer_rate_limited' : (r.status === 401 || r.status === 403) ? 'issuer_unauthorized' : 'issuer_unavailable', 503);
  try { const content = await r.text(); if (content.length > 1048576) throw new Error(); return JSON.parse(content); } catch { throw new Fault('issuer_invalid_response', 502); }
}
export class OfficialIssuer implements Issuer {
  async verify(b: Bundle): Promise<void> {
    // Codex has no identity endpoint the Worker can reach: chatgpt.com answers Workers egress with a 403 page
    // (2026-11-25), and calling it would turn every import and rotation into a refusal. identity() has already
    // checked that the id and access tokens name the same user and workspace and that access is unexpired; a
    // refresh token the issuer no longer accepts surfaces as refresh_rejected at the next renewal.
    if (b.provider === 'codex') return;
    const p = await request('https://api.anthropic.com/api/oauth/profile', b.credential.accessToken, undefined, { 'anthropic-beta': 'oauth-2025-04-20' });
    if (p.account?.uuid !== b.identity.accountUuid || p.organization?.uuid !== b.identity.organizationUuid) throw new Fault('identity_mismatch');
  }
  async refresh(b: Bundle): Promise<Bundle> {
    const next = structuredClone(b); const c = next.credential;
    if (b.provider === 'codex') {
      const p = await request('https://auth.openai.com/oauth/token', undefined, { grant_type: 'refresh_token', refresh_token: c.tokens.refresh_token, client_id: 'app_EMoamEEZ73f0CkXaXp7hrann' });
      if (!p.access_token || !p.refresh_token) throw new Fault('issuer_invalid_response', 502);
      c.tokens.access_token = p.access_token; c.tokens.refresh_token = p.refresh_token; if (p.id_token) c.tokens.id_token = p.id_token; c.last_refresh = new Date().toISOString();
    } else {
      const p = await request('https://platform.claude.com/v1/oauth/token', undefined, { grant_type: 'refresh_token', refresh_token: c.refreshToken, client_id: '9d1c250a-e61b-44d9-88ed-5944d1962f5e', scope: c.scopes.join(' ') });
      if (!p.access_token || !Number.isFinite(p.expires_in) || p.expires_in <= 0) throw new Fault('issuer_invalid_response', 502);
      c.accessToken = p.access_token; c.refreshToken = p.refresh_token || c.refreshToken; c.expiresAt = Date.now() + p.expires_in * 1000;
      if (typeof p.scope === 'string') c.scopes = p.scope.split(' ');
      if (Number.isFinite(p.refresh_token_expires_in)) c.refreshTokenExpiresAt = Date.now() + p.refresh_token_expires_in * 1000;
    }
    identity(next); return next;
  }
}
