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
    if (b.provider === 'codex') { await this.codexUsage(b); return; }
    const p = await request('https://api.anthropic.com/api/oauth/profile', b.credential.accessToken, undefined, { 'anthropic-beta': 'oauth-2025-04-20' });
    if (p.account?.uuid !== b.identity.accountUuid || p.organization?.uuid !== b.identity.organizationUuid) throw new Fault('identity_mismatch');
  }
  private codexUsage(b: Bundle): Promise<any> { return request('https://chatgpt.com/backend-api/wham/usage', b.credential.tokens.access_token, undefined, { 'ChatGPT-Account-Id': b.credential.tokens.account_id, 'User-Agent': 'codex-cli' }); }
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
  async usage(b: Bundle): Promise<any> {
    const windows: any[] = [];
    if (b.provider === 'codex') {
      const p = await this.codexUsage(b);
      for (const [name, value] of Object.entries({ '订阅': p.rate_limit, '代码审查': p.code_review_rate_limit })) {
        for (const [kind, w] of Object.entries((value || {}) as any)) { const v = w as any; if (!kind.endsWith('_window') || !Number.isFinite(v?.used_percent)) continue; windows.push({ label: `${name} · ${Math.round(v.limit_window_seconds / 3600)} 小时`, used_percent: Math.min(100, Math.max(0, v.used_percent)), resets_at: v.reset_at || null }); }
      }
    } else {
      const p = await request('https://api.anthropic.com/api/oauth/usage', b.credential.accessToken, undefined, { 'anthropic-beta': 'oauth-2025-04-20' });
      for (const [name, value] of Object.entries(p)) { const v = value as any; if (!Number.isFinite(v?.utilization)) continue; const labels: Record<string, string> = { five_hour: '5 小时', seven_day: '7 天', seven_day_sonnet: 'Sonnet · 7 天', seven_day_opus: 'Opus · 7 天' }; windows.push({ label: labels[name] || name.replaceAll('_', ' '), used_percent: Math.min(100, Math.max(0, v.utilization)), resets_at: v.resets_at ? Math.floor(Date.parse(v.resets_at) / 1000) : null }); }
    }
    if (!windows.length) throw new Fault('usage_unavailable', 503);
    return { windows, checked_at: Math.floor(Date.now() / 1000) };
  }
}
