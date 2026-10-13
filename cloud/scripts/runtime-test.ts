import { createRequire } from 'node:module';
import assert from 'node:assert/strict';
import { mkdtemp, readdir, readFile, rm } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { tmpdir } from 'node:os';
const req = createRequire(resolve(process.argv[2], 'package.json'));
const cached = createRequire(req.resolve('wrangler/package.json'));
const { Miniflare, Log, LogLevel, convertV4MiniflareOptions, Response: MockResponse } = cached('miniflare');
const directory = await mkdtemp(join(tmpdir(), 'authmanager-cloud-runtime-'));
const key = await crypto.subtle.generateKey({ name: 'RSASSA-PKCS1-v1_5', modulusLength: 2048, publicExponent: new Uint8Array([1,0,1]), hash: 'SHA-256' }, true, ['sign','verify']);
const jwk = { ...await crypto.subtle.exportKey('jwk', key.publicKey), kid: 'synthetic-key' };
const b64 = (x: unknown) => Buffer.from(JSON.stringify(x)).toString('base64url');
const partial = `${b64({ alg:'RS256', kid:'synthetic-key' })}.${b64({ iss:'https://synthetic.cloudflareaccess.com', aud:['synthetic-aud'], email:'test@example.invalid', exp:Math.floor(Date.now()/1000)+300 })}`;
const signature = Buffer.from(await crypto.subtle.sign('RSASSA-PKCS1-v1_5', key.privateKey, Buffer.from(partial))).toString('base64url');
const jwt = `${partial}.${signature}`;
const credential = { provider:'claude', credential:{ accessToken:'SYNTHETIC-private-access-runtime', refreshToken:'SYNTHETIC-private-refresh-runtime', scopes:['user:inference','user:profile'], expiresAt:Date.now()+3600000, subscriptionType:'pro' }, identity:{ accountUuid:'synthetic-user', organizationUuid:'synthetic-org', emailAddress:'test@example.invalid' } };
let upstreamCalls = 0;
const worker = {
  name:'authmanager-test',
  scriptPath: resolve('dist/cloud/worker.js'), modules:true, compatibilityDate:'2026-09-18',
  durableObjects:{ VAULT:{ className:'Vault', useSQLite:true } },
  bindings:{ VAULT_KEY:Buffer.alloc(32,7).toString('base64'), ACCESS_TEAM:'synthetic', ACCESS_AUD:'synthetic-aud', OWNER_EMAIL:'test@example.invalid' },
  outboundService:(r:Request) => {
    upstreamCalls++;
    if(r.url==='https://synthetic.cloudflareaccess.com/cdn-cgi/access/certs') return MockResponse.json({keys:[jwk]});
    if(r.url==='https://api.anthropic.com/api/oauth/profile') return MockResponse.json({account:{uuid:'synthetic-user'},organization:{uuid:'synthetic-org'}});
    throw new Error('Unexpected outbound URL; real network is forbidden');
  }
};
const options = convertV4MiniflareOptions({ workers:[worker], durableObjectsPersist:directory, cf:false, log:new Log(LogLevel.ERROR) });
options.resourcePersistencePath=directory;
let mf = new Miniflare(options);
async function call(path:string,body:unknown,token?:string,admin=false):Promise<Response> {
  return mf.dispatchFetch(`https://auth.example.invalid${path}`, { method:'POST',headers:{'Content-Type':'application/json',...(token?{Authorization:`Bearer ${token}`} : {}),...(admin?{'Cf-Access-Jwt-Assertion':jwt}: {})},body:JSON.stringify(body) });
}
try {
  assert.equal((await mf.dispatchFetch('https://auth.example.invalid/health')).status,200);
  assert.equal((await call('/v1/accounts',{})).status,401);
  assert.equal((await call('/v1/admin.approve',{},undefined,true)).status,403);
  assert.equal((await call('/admin/api/admin.pairs',{})).status,401);
  const pair=await (await call('/v1/pair.begin',{proof:'b'.repeat(64),label:'Synthetic Mac'})).json();
  assert.equal((await call('/admin/api/admin.approve',{id:pair.id,code:pair.code},undefined,true)).status,200,`synthetic outbound calls: ${upstreamCalls}`);
  const paired=await (await call('/v1/pair.poll',{id:pair.id},'b'.repeat(64))).json();
  assert.equal(paired.state,'paired');
  const imported=await (await call('/v1/import',{operation:'synthetic-runtime-import',label:'Synthetic Claude',bundle:credential},paired.device_token)).json();
  assert.equal(imported.state,'awaiting_handoff');
  assert.equal((await call('/v1/lease',{id:imported.id},paired.device_token)).status,409);
  assert.equal((await call('/v1/import.commit',{id:imported.id,operation:'synthetic-runtime-import'},paired.device_token)).status,200);
  const lease=await (await call('/v1/lease',{id:imported.id},paired.device_token)).json();
  assert.equal(lease.access_token,credential.credential.accessToken);
  assert.equal(JSON.stringify(lease).includes(credential.credential.refreshToken),false);
  const list=await (await call('/v1/accounts',{},paired.device_token)).text();
  assert.equal(list.includes(credential.credential.accessToken),false);
  await mf.dispose();
  let persistedFiles=0;
  async function scan(path:string):Promise<void> { for(const e of await readdir(path,{withFileTypes:true})){const p=join(path,e.name);if(e.isDirectory())await scan(p);else {persistedFiles++;const value=await readFile(p);assert.equal(value.includes(credential.credential.refreshToken),false);assert.equal(value.includes(paired.device_token),false);}} }
  await scan(directory);
  assert.ok(persistedFiles>0,'Persistence test must examine actual files');
  mf=new Miniflare(options);
  const restarted=await call('/v1/accounts',{},paired.device_token);
  assert.equal(restarted.status,200,await restarted.clone().text());
  assert.equal((await restarted.json()).accounts[0].id,imported.id);
  assert.equal((await call('/admin/api/admin.revoke',{id:paired.device_id},undefined,true)).status,200);
  assert.equal((await call('/v1/lease',{id:imported.id},paired.device_token)).status,401);
  console.log(JSON.stringify({runtime:'workerd',pairing:true,sourceHandoff:true,encryptedStorage:true,restart:true,revocation:true,realNetworkRequests:0,syntheticUpstreamCalls:upstreamCalls}));
} finally { await mf.dispose(); await rm(directory,{recursive:true,force:true}); }
