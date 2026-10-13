import assert from "node:assert/strict";
import { after, test } from "node:test";
import { cp, mkdtemp, rm, writeFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BrokerClient, type Access, type Account, type Directory } from "./authmanager-codex/client.ts";

// Exercise the installed pi runtime with isolated metadata and an in-memory
// credential store. No installed extension/config, real socket, or model is used.
const piRoot = join(execFileSync("npm", ["root", "-g"], { encoding: "utf8" }).trim(), "@earendil-works/pi-coding-agent");
const aiRoot = `${piRoot}/node_modules/@earendil-works/pi-ai/dist`;
const root = await mkdtemp(join(tmpdir(), "authmanager-pi-provider-test-"));
after(async () => { await rm(root, { recursive: true, force: true }); });
for (const file of ["index.ts", "client.ts", "session.ts", "names.ts"]) {
	await cp(new URL(`./authmanager-codex/${file}`, import.meta.url), join(root, file));
}
await writeFile(join(root, "bindings.json"), "{}\n");
const { createJiti } = await import(`${piRoot}/node_modules/jiti/lib/jiti.mjs`);
const jiti = createJiti(import.meta.url, { fsCache: false, alias: {
	"@earendil-works/pi-ai/providers/all": `${aiRoot}/providers/all.js`,
	"@earendil-works/pi-ai": `${aiRoot}/index.js`,
} });
const { registerAuthManager } = await jiti.import(join(root, "index.ts")) as any;
const { ProviderNames } = await jiti.import(join(root, "names.ts")) as any;
const { ModelRuntime } = await import(`${piRoot}/dist/core/model-runtime.js`);
const { ModelConfig } = await import(`${piRoot}/dist/core/model-config.js`);
const { RuntimeCredentials } = await import(`${piRoot}/dist/core/runtime-credentials.js`);
const { InMemoryCodingAgentModelsStore } = await import(`${piRoot}/dist/core/models-store.js`);
const { InMemoryCredentialStore } = await import(`${aiRoot}/auth/credential-store.js`);
const { refreshModelCatalogs } = await import(`${piRoot}/dist/modes/interactive/model-catalog-refresh.js`);

const DEFAULT = "authmanager-codex";
const A = "00000000-0000-4000-8000-000000000001";
const B = "00000000-0000-4000-8000-000000000002";
const C = "00000000-0000-4000-8000-000000000003";
const readable = new Map([[A, `${DEFAULT}-nova`], [B, `${DEFAULT}-orbit`], [C, `${DEFAULT}-lab-team`]]);
const account = (id: string, label: string, current = false): Account => ({ id, label, current, state: "delegated", automatic: true });

class SyntheticBroker extends BrokerClient {
	directory: Directory = { accounts: [account(A, "Nova", true), account(B, "orbit"), account(C, "lab team")], recovery_needed: false };
	offline = false;
	failNextDirectory = false;
	directoryCalls = 0;
	latencyMs = 0;
	accessCalls: string[] = [];
	generation = 1;
	constructor() { super(join(root, "never-connected.sock")); }
	async accounts(signal?: AbortSignal): Promise<Directory> {
		signal?.throwIfAborted();
		this.directoryCalls++;
		if (this.latencyMs) await new Promise((resolve) => setTimeout(resolve, this.latencyMs));
		if (this.offline || this.failNextDirectory) {
			this.failNextDirectory = false;
			throw new Error("无法连接 AuthManager（合成服务离线）");
		}
		return structuredClone(this.directory);
	}
	async access(id: string, signal?: AbortSignal): Promise<Access> {
		signal?.throwIfAborted();
		this.accessCalls.push(id);
		if (this.offline) throw new Error("无法连接 AuthManager（合成服务离线）");
		const selected = this.directory.accounts.find((entry) => entry.id === id);
		assert.ok(selected, "the extension must request an existing, explicit account");
		return { state: "ready", account_id: id, label: selected.label,
			access_token: `SYNTHETIC-${id}-${this.generation}`, expires_at: Math.floor(Date.now() / 1000) + 3600 };
	}
	select(id?: string): void {
		for (const entry of this.directory.accounts) entry.current = entry.id === id;
	}
}

type Handler = (...args: any[]) => unknown;
let fixtureId = 0;
async function fixture(client = new SyntheticBroker(), namesDirectory = join(root, `names-${fixtureId++}`)): Promise<any> {
	// The real constructor is used instead of create(), so no builtin ambient
	// auth checks, auth.json, models.json, or remote catalog can be consulted.
	const runtime = new ModelRuntime(new RuntimeCredentials(new InMemoryCredentialStore()),
		await ModelConfig.load(undefined), undefined, new InMemoryCodingAgentModelsStore(), [], false);
	const events = new Map<string, Handler>();
	const commands = new Map<string, { handler: Handler }>();
	const notices: { message: string; type: string }[] = [];
	const context = { ui: { setStatus(): void {}, notify(message: string, type: string): void { notices.push({ message, type }); } } };
	const refresh = async (): Promise<void> => {
		const result = await refreshModelCatalogs(runtime, new AbortController().signal);
		assert.equal(result.aborted, false);
		assert.equal(result.errors.size, 0);
		assert.equal(runtime.getError(), undefined);
	};
	await registerAuthManager({
		registerProvider: (provider: any): void => runtime.registerNativeProvider(provider),
		unregisterProvider: (id: string): void => runtime.unregisterProvider(id),
		on: (event: string, handler: Handler): void => { events.set(event, handler); },
		registerCommand: (name: string, command: { handler: Handler }): void => { commands.set(name, command); },
	}, client, new ProviderNames(namesDirectory));
	await refresh();
	return {
		client, runtime, notices, namesDirectory, refresh,
		modelCount: runtime.getModels(DEFAULT).length,
		available: (id: string): number => runtime.getAvailableSnapshot().filter((model: any) => model.provider === id).length,
		emit: async (event: string): Promise<void> => { await events.get(event)!({}, context); },
		update: async (): Promise<void> => { await commands.get("authmanager")!.handler("", context); await refresh(); },
	};
}

test("default remains listed through failed directory probes without resolving credentials", async () => {
	const f = await fixture();
	assert.ok(f.modelCount > 0);
	assert.equal(f.available(DEFAULT), f.modelCount);
	const before = f.client.directoryCalls;
	assert.equal((await f.runtime.checkAuth(DEFAULT)).source, "AuthManager（请求时连接）");
	assert.equal(f.client.directoryCalls, before, "default availability must not probe the broker");
	f.client.failNextDirectory = true;
	await f.refresh();
	assert.equal(f.available(DEFAULT), f.modelCount);
	f.client.offline = true;
	await f.refresh();
	assert.equal(f.available(DEFAULT), f.modelCount);
	assert.equal(f.runtime.getAvailableSnapshot().length, f.modelCount);
	assert.deepEqual(f.client.accessCalls, []);
	assert.equal(f.runtime.getProvider(DEFAULT).auth.oauth, undefined);
});

test("overlapping availability checks share one broker directory request", async () => {
	const f = await fixture();
	f.client.latencyMs = 20;
	const names = [...readable.values()];
	const before = f.client.directoryCalls;
	const checks = await Promise.all(Array.from({ length: 24 }, (_, i) => f.runtime.checkAuth(names[i % names.length])));
	assert.ok(checks.every((result: any) => result?.source === "AuthManager"));
	assert.equal(f.client.directoryCalls - before, 1);
	f.client.offline = true;
	await f.refresh();
	assert.equal(f.runtime.getAvailableSnapshot().length, f.modelCount, "a settled request is not reused");
});

test("default resolution rejects offline, missing current and recovery without taking another account", async () => {
	const f = await fixture();
	f.client.offline = true;
	await assert.rejects(f.runtime.getAuth(DEFAULT), /无法连接 AuthManager/);
	f.client.offline = false;
	f.client.select();
	await assert.rejects(f.runtime.getAuth(DEFAULT), /选择唯一/);
	f.client.select(A);
	f.client.directory.recovery_needed = true;
	await assert.rejects(f.runtime.getAuth(DEFAULT), /恢复未完成/);
	assert.deepEqual(f.client.accessCalls, []);
	await f.refresh();
	assert.equal(f.available(DEFAULT), f.modelCount);
});

test("readable providers and hidden UUID aliases resolve the same identities after rename and reload", async () => {
	const f = await fixture();
	const assertBindings = async (current: any): Promise<void> => {
		assert.deepEqual(new Set(current.runtime.getAvailableSnapshot().map((model: any) => model.provider)), new Set([DEFAULT, ...readable.values()]));
		for (const [id, name] of readable) {
			const old = `${DEFAULT}-${id}`;
			assert.equal(current.available(name), current.modelCount);
			assert.equal(current.available(old), 0);
			const restored = current.runtime.getModels(old)[0];
			assert.ok(restored);
			assert.equal(current.runtime.getModel(old, restored.id), restored);
			assert.equal(current.runtime.hasConfiguredAuth(old), true);
			const namedAuth = await current.runtime.getAuth(name);
			const oldAuth = await current.runtime.getAuth(restored);
			assert.equal(namedAuth.auth.apiKey, `SYNTHETIC-${id}-1`);
			assert.equal(oldAuth.auth.apiKey, namedAuth.auth.apiKey);
		}
	};
	await assertBindings(f);
	for (const entry of f.client.directory.accounts) entry.label = `Renamed ${entry.id}`;
	await f.update();
	await assertBindings(f);
	const reloaded = await fixture(f.client, f.namesDirectory);
	await assertBindings(reloaded);
	assert.deepEqual(f.client.accessCalls, [A, A, B, B, C, C, A, A, B, B, C, C, A, A, B, B, C, C]);
});

test("offline startup reconnects through the command and session event while preserving the default entry", async () => {
	const client = new SyntheticBroker();
	client.offline = true;
	const f = await fixture(client);
	assert.equal(f.runtime.getAvailableSnapshot().length, f.modelCount);
	assert.equal(f.available(DEFAULT), f.modelCount);
	await assert.rejects(f.runtime.getAuth(DEFAULT), /无法连接 AuthManager/);
	client.offline = false;
	client.select(B);
	await f.update();
	assert.equal(f.notices.at(-1).type, "info");
	assert.equal(f.runtime.getAvailableSnapshot().length, f.modelCount * 4);
	assert.equal((await f.runtime.getAuth(DEFAULT)).auth.apiKey, `SYNTHETIC-${B}-1`);
	client.offline = true;
	await f.emit("session_start");
	await f.refresh();
	assert.equal(f.available(DEFAULT), f.modelCount);
	client.offline = false;
	client.select(C);
	await f.emit("session_start");
	await f.refresh();
	assert.equal(f.runtime.getAvailableSnapshot().length, f.modelCount * 4);
	assert.equal((await f.runtime.getAuth(DEFAULT)).auth.apiKey, `SYNTHETIC-${C}-1`);
	assert.deepEqual(client.accessCalls, [B, C]);
});

test("default account stays pinned during a run and follows the user's switch only after settlement", async () => {
	const f = await fixture();
	await f.emit("before_agent_start");
	assert.equal((await f.runtime.getAuth(DEFAULT)).auth.apiKey, `SYNTHETIC-${A}-1`);
	f.client.select(B);
	f.client.generation = 2;
	await f.update();
	assert.equal((await f.runtime.getAuth(DEFAULT)).auth.apiKey, `SYNTHETIC-${A}-2`);
	assert.equal((await f.runtime.getAuth(`${DEFAULT}-${A}`)).auth.apiKey, `SYNTHETIC-${A}-2`);
	await assert.rejects(f.runtime.getAuth(readable.get(B)), /固定账号/);
	assert.deepEqual(f.client.accessCalls, [A, A, A]);
	await f.emit("agent_settled");
	await f.emit("before_agent_start");
	assert.equal((await f.runtime.getAuth(DEFAULT)).auth.apiKey, `SYNTHETIC-${B}-2`);
	await f.emit("session_shutdown");
	f.client.select(C);
	await f.emit("before_agent_start");
	assert.equal((await f.runtime.getAuth(DEFAULT)).auth.apiKey, `SYNTHETIC-${C}-2`);
	assert.deepEqual(f.client.accessCalls, [A, A, A, B, C]);
});
