import { Fault } from './core.ts';
export type AccessConfig = { ACCESS_TEAM: string; ACCESS_AUD: string; OWNER_EMAIL: string };
function decode(s: string): Uint8Array { return Uint8Array.from(atob(s.replaceAll('-', '+').replaceAll('_', '/')), c => c.charCodeAt(0)); }
export async function owner(request: Request, env: AccessConfig, fetcher: typeof fetch = fetch): Promise<void> {
  if (!/^[a-z0-9-]+$/.test(env.ACCESS_TEAM || '') || !env.ACCESS_AUD || !env.OWNER_EMAIL) throw new Fault('deployment_not_configured', 503);
  try {
    const token = request.headers.get('Cf-Access-Jwt-Assertion') || ''; const [head, payload, sig, extra] = token.split('.');
    if (!sig || extra) throw new Error();
    const h = JSON.parse(new TextDecoder().decode(decode(head))); const p = JSON.parse(new TextDecoder().decode(decode(payload)));
    const issuer = `https://${env.ACCESS_TEAM}.cloudflareaccess.com`;
    if (h.alg !== 'RS256' || p.iss !== issuer || !Array.isArray(p.aud) || !p.aud.includes(env.ACCESS_AUD) || typeof p.exp !== 'number' || !Number.isFinite(p.exp) || p.exp <= Date.now() / 1000 || (p.nbf !== undefined && (typeof p.nbf !== 'number' || !Number.isFinite(p.nbf) || p.nbf > Date.now() / 1000)) || p.email?.toLowerCase() !== env.OWNER_EMAIL.toLowerCase()) throw new Error();
    const r = await fetcher(`${issuer}/cdn-cgi/access/certs`, { redirect: 'manual', signal: AbortSignal.timeout(5000) });
    if (!r.ok) throw new Error(); const jwks = await r.json() as any; const jwk = jwks.keys.find((k: any) => k.kid === h.kid && k.kty === 'RSA'); if (!jwk) throw new Error();
    const key = await crypto.subtle.importKey('jwk', jwk, { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' }, false, ['verify']);
    if (!await crypto.subtle.verify('RSASSA-PKCS1-v1_5', key, decode(sig), new TextEncoder().encode(`${head}.${payload}`))) throw new Error();
  } catch (e) { if (e instanceof Fault) throw e; throw new Fault('owner_unauthorized', 401); }
}
