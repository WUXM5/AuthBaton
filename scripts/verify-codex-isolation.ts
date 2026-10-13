// P0 only: installed CLI, synthetic credentials, isolated CODEX_HOME, no network.
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

type Message = { id?: number; result?: { account?: { email?: string; type?: string } }; error?: unknown };
const binary = process.argv[2] ?? '/opt/homebrew/bin/codex';
assert.equal(execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim(), 'codex-cli 0.155.1', 'P0 contract requires the reviewed CLI version');
const root = mkdtempSync(join(tmpdir(), 'authmanager-codex-p0-'));

function fixture(profile: string, identity: string, expired: boolean): string {
    const jwt = (value: unknown): string => `${Buffer.from('{"alg":"none"}').toString('base64url')}.${Buffer.from(JSON.stringify(value)).toString('base64url')}.c3ludGhldGlj`;
    const claims = { email: `${identity}@example.invalid`, exp: expired ? 1 : Math.floor(Date.now() / 1000) + 3600,
        'https://api.openai.com/auth': { chatgpt_plan_type: 'pro', chatgpt_user_id: identity, chatgpt_account_id: `workspace-${identity}` } };
    const payload = JSON.stringify({ auth_mode: 'chatgpt', OPENAI_API_KEY: null,
        tokens: { id_token: jwt(claims), access_token: jwt(claims), refresh_token: 'synthetic-never-valid', account_id: `workspace-${identity}` },
        last_refresh: new Date().toISOString() });
    writeFileSync(join(profile, 'auth.json'), payload, { mode: 0o600 });
    return payload;
}

async function server(profile: string) {
    const environment: NodeJS.ProcessEnv = { PATH: '/usr/bin:/bin', CODEX_HOME: profile };
    for (const key of ['HOME', 'USER', 'LANG', 'TMPDIR']) if (process.env[key]) environment[key] = process.env[key];
    const child = spawn('/usr/bin/sandbox-exec', ['-p', '(version 1)(allow default)(deny network*)', binary,
        '-c', 'cli_auth_credentials_store="file"', 'app-server'],
        { cwd: profile, env: environment, detached: true, stdio: ['pipe', 'pipe', 'ignore'] });
    const ended = once(child, 'exit');
    const pending = new Map<number, { resolve: (message: Message) => void; reject: (error: Error) => void }>();
    const lines = createInterface({ input: child.stdout });
    lines.on('line', (line: string) => {
        let message: Message;
        try { message = JSON.parse(line); } catch { return; }
        if (typeof message.id === 'number') pending.get(message.id)?.resolve(message);
    });
    child.on('exit', () => { for (const waiter of pending.values()) waiter.reject(new Error('CLI exited before P0 response')); });
    let serial = 0;
    async function request(method: string, params: unknown): Promise<Message> {
        const id = ++serial;
        const response = new Promise<Message>((resolve, reject) => { pending.set(id, { resolve, reject }); });
        const timer = setTimeout(() => pending.get(id)?.reject(new Error('P0 response timed out')), 15000);
        child.stdin.write(`${JSON.stringify({ id, method, params })}\n`);
        try { const value = await response; assert.equal(value.error, undefined, 'official RPC rejected the synthetic read'); return value; }
        finally { clearTimeout(timer); pending.delete(id); }
    }
    async function close(): Promise<void> {
        lines.close();
        if (child.exitCode === null && child.signalCode === null) {
            try { process.kill(-child.pid!, 'SIGKILL'); } catch { /* already exited */ }
        }
        await ended;
    }
    try {
        await request('initialize', { clientInfo: { name: 'authmanager_p0', version: '0.2.0' } });
        child.stdin.write('{"method":"initialized"}\n');
    } catch (error) { await close(); throw error; }
    return { request, close };
}

try {
    const a = join(root, 'a'), b = join(root, 'b');
    mkdirSync(a, { mode: 0o700 }); mkdirSync(b, { mode: 0o700 });
    const originalA = fixture(a, 'alpha', false), originalB = fixture(b, 'beta', true);
    const clientA = await server(a);
    try {
        const clientB = await server(b);
        try {
            const accountA = await clientA.request('account/read', { refreshToken: false });
            const accountB = await clientB.request('account/read', { refreshToken: false });
            assert.equal(accountA.result?.account?.email, 'alpha@example.invalid');
            assert.equal(accountB.result?.account?.email, 'beta@example.invalid');
            assert.equal(readFileSync(join(a, 'auth.json'), 'utf8'), originalA);
            assert.equal(readFileSync(join(b, 'auth.json'), 'utf8'), originalB);
            fixture(a, 'gamma', false);
            const cached = await clientA.request('account/read', { refreshToken: false });
            assert.equal(cached.result?.account?.email, 'alpha@example.invalid');
        } finally { await clientB.close(); }
    } finally { await clientA.close(); }
    const reopened = await server(a);
    try {
        const latest = await reopened.request('account/read', { refreshToken: false });
        assert.equal(latest.result?.account?.email, 'gamma@example.invalid');
    } finally { await reopened.close(); }
    console.log(JSON.stringify({ cli: '0.155.1', assertions: 6, passed: 6, syntheticOnly: true,
        networkDeniedBySandbox: true, realLoginRefreshCalls: 0,
        findings: ['profiles are independent', 'local read accepts expired synthetic token', 'read does not change credentials', 'existing process caches identity', 'new process loads changed identity'] }));
} finally { rmSync(root, { recursive: true, force: true }); }
