import assert from "node:assert/strict";
import { after, test } from "node:test";
import { mkdtemp, chmod, rm } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BrokerClient, RunBinding } from "./authmanager-codex/client.ts";
import { accountSession } from "./authmanager-codex/session.ts";

const root = await mkdtemp(join(tmpdir(), "authmanager-pi-test-"));
await chmod(root, 0o700);
let counter = 0;
after(async () => { await rm(root, { recursive: true, force: true }); });
async function fixture(reply: (value: any) => unknown): Promise<{ client: BrokerClient; close: () => Promise<void> }> {
	const path = join(root, `${counter++}.sock`);
	const server = createServer((socket) => {
		let buffer = "";
		socket.on("data", (chunk) => {
			buffer += chunk.toString(); if (!buffer.includes("\n")) return;
			const request = JSON.parse(buffer.split("\n")[0]);
			socket.end(`${JSON.stringify({ version: 1, id: request.id, ...reply(request) })}\n`);
		});
	});
	await new Promise<void>((resolve) => server.listen(path, resolve)); await chmod(path, 0o600);
	return { client: new BrokerClient(path), close: () => new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve())) };
}
const account = (id: string, current: boolean) => ({ id, label: id, current, automatic: true, state: "healthy" });
const token = (id: string, suffix = "") => ({ state: "ready", account_id: id, label: id, access_token: `SYNTHETIC-${id}${suffix}`, expires_at: Math.floor(Date.now()/1000)+3600 });

test("transport partitions different users in one workspace without resetting on token rotation", () => {
	const jwt = (user: string, exp: number): string => `x.${Buffer.from(JSON.stringify({ exp, "https://api.openai.com/auth": { chatgpt_user_id: user, chatgpt_account_id: "team" } })).toString("base64url")}.x`;
	const a = accountSession({ sessionId: "same-pi-session", apiKey: jwt("a", 1) })!;
	const b = accountSession({ sessionId: "same-pi-session", apiKey: jwt("b", 1) })!;
	assert.notEqual(a.sessionId, b.sessionId);
	assert.equal(a.sessionId, accountSession({ sessionId: "same-pi-session", apiKey: jwt("a", 2) })!.sessionId);
	assert.throws(() => accountSession({ sessionId: "a", apiKey: "SYNTHETIC_SECRET" }), /身份无效/);
});

test("pins identity until settled but obtains fresh token on every request", async () => {
	let current = "a", generation = 1;
	const f = await fixture((r) => ({ result: r.method === "accounts" ? { accounts: [account("a", current === "a"), account("b", current === "b")], recovery_needed: false } : token(r.account_id, String(generation)) }));
	try {
		const binding = new RunBinding(f.client); binding.begin();
		assert.equal((await binding.access()).access_token, "SYNTHETIC-a1");
		current = "b"; generation = 2;
		assert.equal((await binding.access()).access_token, "SYNTHETIC-a2");
		await assert.rejects(binding.access("b"), /固定账号/);
		binding.end(); binding.begin(); assert.equal((await binding.access()).account_id, "b");
	} finally { await f.close(); }
});
test("waits for maintenance using same account and never receives a refresh token", async () => {
	let attempts = 0;
	const f = await fixture((r) => { assert.equal(r.account_id, "a"); return { result: ++attempts === 1 ? { state: "waiting", account_id: "a", retry_after_ms: 1 } : token("a") }; });
	try { assert.equal((await f.client.access("a")).account_id, "a"); assert.equal(attempts, 2); }
	finally { await f.close(); }
});
test("rejects wrong identity, expired material, and malformed wire responses without echoing tokens", async () => {
	for (const result of [token("wrong"), { ...token("a"), expires_at: 1 }, { ...token("a"), access_token: "SYNTHETIC SECRET" }]) {
		const f = await fixture(() => ({ result }));
		try { await assert.rejects(f.client.access("a"), (error: Error) => !error.message.includes("SYNTHETIC") && /无效/.test(error.message)); }
		finally { await f.close(); }
	}
	const f = await fixture(() => ({ id: "wrong", result: token("a") }));
	try { await assert.rejects(f.client.access("a"), /无效/); } finally { await f.close(); }
});
test("rejects sockets readable by other users and reports service absence", async () => {
	const f = await fixture(() => ({ result: token("a") }));
	try { await chmod(f.client.path, 0o666); await assert.rejects(f.client.access("a"), /权限或所有者异常/); }
	finally { await f.close(); }
	await assert.rejects(f.client.access("a"), /无法连接/);
});
test("a connection dropped before the reply is not reported as service absence", async () => {
	const path = join(root, `${counter++}.sock`);
	const server = createServer((socket) => socket.destroy());
	await new Promise<void>((resolve) => server.listen(path, resolve)); await chmod(path, 0o600);
	try {
		for (let i = 0; i < 20; i++) await assert.rejects(new BrokerClient(path).accounts(), (error: Error) => /断开了连接/.test(error.message) && !/无法连接/.test(error.message));
	} finally { await new Promise<void>((resolve) => server.close(() => resolve())); }
});
test("stops waiting after cancellation and preserves explicit maintenance errors", async () => {
	const f = await fixture(() => ({ result: { state: "waiting", account_id: "a", retry_after_ms: 750 } }));
	try { await assert.rejects(f.client.access("a", AbortSignal.timeout(30))); }
	finally { await f.close(); }
	const e = await fixture(() => ({ error: "需要在 AuthManager 重新登录" }));
	try { await assert.rejects(e.client.access("a"), /重新登录/); } finally { await e.close(); }
});
test("missing default or switch recovery cannot fall back to another account", async () => {
	for (const recovery of [false, true]) {
		const f = await fixture(() => ({ result: { accounts: [account("a", false)], recovery_needed: recovery } }));
		try { await assert.rejects(new RunBinding(f.client).access(), /选择|恢复/); } finally { await f.close(); }
	}
});

// Use the already-installed pi runtime, never install dependencies or invoke a real model.
test("real pi auth resolution works without stored OAuth and never registers a refresh handler", async () => {
	const piRoot = join(execFileSync("npm", ["root", "-g"], { encoding: "utf8" }).trim(), "@earendil-works/pi-coding-agent");
	const { createJiti } = await import(`${piRoot}/node_modules/jiti/lib/jiti.mjs`);
	const jiti = createJiti(import.meta.url, { fsCache: false, alias: {
		"@earendil-works/pi-ai/providers/all": `${piRoot}/node_modules/@earendil-works/pi-ai/dist/providers/all.js`,
		"@earendil-works/pi-ai": `${piRoot}/node_modules/@earendil-works/pi-ai/dist/index.js`,
		"@earendil-works/pi-tui": `${piRoot}/node_modules/@earendil-works/pi-tui/dist/index.js`,
		typebox: `${piRoot}/node_modules/typebox/build/index.mjs`,
	} });
	const { registerAuthManager } = await jiti.import(new URL("./authmanager-codex/index.ts", import.meta.url).href) as any;
	const { createModels } = await import(`${piRoot}/node_modules/@earendil-works/pi-ai/dist/models.js`);
	const models = createModels();
	const events = new Map(); let accessCalls = 0;
	const f = await fixture((r) => {
		if (r.method === "accounts") return { result: { accounts: [account("a", true)], recovery_needed: false } };
		accessCalls++; return { result: token("a") };
	});
	try {
		await registerAuthManager({ registerProvider: (p: any) => models.setProvider(p), unregisterProvider: (id: string) => models.deleteProvider(id), on: (name: string, callback: any) => events.set(name, callback), registerCommand: () => {}, registerTool: () => {} }, f.client);
		assert.equal(models.getProvider("authmanager-codex").auth.oauth, undefined);
		assert.ok((await models.getAvailable("authmanager-codex")).length > 0);
		assert.equal(accessCalls, 0, "model discovery must not fetch credentials or renew");
		events.get("before_agent_start")();
		assert.equal((await models.getAuth("authmanager-codex")).auth.apiKey, "SYNTHETIC-a");
		assert.equal((await models.getAuth("authmanager-codex-a")).auth.apiKey, "SYNTHETIC-a");
		assert.equal(accessCalls, 2);
		events.get("agent_settled")();
	} finally { await f.close(); }
});
