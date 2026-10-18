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
for (const file of ["index.ts", "client.ts", "session.ts", "names.ts", "web-run.ts"]) {
	await cp(new URL(`./authmanager-codex/${file}`, import.meta.url), join(root, file));
}
await writeFile(join(root, "bindings.json"), "{}\n");
const { createJiti } = await import(`${piRoot}/node_modules/jiti/lib/jiti.mjs`);
const jiti = createJiti(import.meta.url, { fsCache: false, alias: {
	"@earendil-works/pi-ai/providers/all": `${aiRoot}/providers/all.js`,
	"@earendil-works/pi-ai": `${aiRoot}/index.js`,
	"@earendil-works/pi-tui": `${piRoot}/node_modules/@earendil-works/pi-tui/dist/index.js`,
	typebox: `${piRoot}/node_modules/typebox/build/index.mjs`,
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
	const events = new Map<string, Handler[]>();
	const tools = new Map<string, any>();
	let active: string[] = ["read"];
	const activeChanges: string[][] = [];
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
		on: (event: string, handler: Handler): void => { events.set(event, [...(events.get(event) ?? []), handler]); },
		registerTool: (tool: any): void => { tools.set(tool.name, tool); },
		getActiveTools: (): string[] => [...active],
		setActiveTools: (names: string[]): void => { active = [...names]; activeChanges.push(active); },
		registerCommand: (name: string, command: { handler: Handler }): void => { commands.set(name, command); },
	}, client, new ProviderNames(namesDirectory));
	await refresh();
	return {
		client, runtime, notices, namesDirectory, refresh, tools, activeChanges,
		active: (): string[] => active,
		modelCount: runtime.getModels(DEFAULT).length,
		available: (id: string): number => runtime.getAvailableSnapshot().filter((model: any) => model.provider === id).length,
		emit: async (event: string, payload = {}, extra = {}): Promise<void> => { for (const handler of events.get(event)!) await handler(payload, { ...context, ...extra }); },
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

test("web_run is active only while an AuthManager model is selected", async () => {
	const f = await fixture();
	assert.equal(f.tools.get("web_run").defaultActive, false);
	await f.emit("session_start", {}, { model: { provider: DEFAULT } });
	assert.deepEqual(f.active(), ["read", "web_run"]);
	await f.emit("model_select", { model: { provider: readable.get(B) } });
	await f.emit("model_select", { model: { provider: "openai-codex" } });
	assert.deepEqual(f.active(), ["read"]);
	await f.emit("session_start", {}, { model: undefined });
	assert.deepEqual(f.activeChanges, [["read", "web_run"], ["read"]], "unchanged selections leave pi's tools alone");
});

test("web_run posts the commands to Codex's standalone search with the model's own account", async (t) => {
	const f = await fixture();
	const claims = { "https://api.openai.com/auth": { chatgpt_account_id: "workspace", chatgpt_user_id: "user" } };
	const jwt = (id: string): string => `e30.${Buffer.from(JSON.stringify({ ...claims, sub: id })).toString("base64url")}.synthetic`;
	f.client.access = async (id: string) => { f.client.accessCalls.push(id); return { state: "ready", account_id: id, label: id, access_token: jwt(id), expires_at: 0 }; };
	const requests: { url: string; init: any }[] = [];
	let reply = (): Response => Response.json({ encrypted_output: "x", output: "Result text citeturn0search0", results: [{ type: "text_result", ref_id: "turn0search0", title: "Title", url: "https://example.test/a", snippet: "s" }, { type: "text_result", ref_id: "turn0view0", title: "Internal Error" }] });
	t.mock.method(globalThis, "fetch", async (url: string, init: any) => { requests.push({ url, init }); return reply(); });
	const text = (role: string, value: string, extra: object[] = []): object => ({ type: "message", message: { role, content: role === "user" ? value : [{ type: "text", text: value }, ...extra] } });
	const branch = [text("user", "q0"), text("assistant", "a0"), text("user", "q1"), { type: "custom" }, text("assistant", "a1", [{ type: "toolCall" }]), text("user", "q2")];
	const ctx = (provider: string) => ({ model: { provider, id: "gpt-6.1-sol" }, sessionManager: { getSessionId: () => "session-1", getBranch: () => branch } });
	const params = { search_query: [{ q: "DeepSeek news" }], open: [{ ref_id: "turn0search0" }] };
	const result = await f.tools.get("web_run").execute("call-1", params, undefined, undefined, ctx(readable.get(B)!));
	assert.deepEqual(result, { content: [{ type: "text", text: "Result text citeturn0search0" }], details: { results: [{ title: "Title", url: "https://example.test/a" }] } });
	assert.equal(requests[0].url, "https://chatgpt.com/backend-api/codex/alpha/search");
	assert.deepEqual(requests[0].init.headers, { authorization: `Bearer ${jwt(B)}`, "chatgpt-account-id": "workspace", originator: "pi", "content-type": "application/json" });
	assert.deepEqual(JSON.parse(requests[0].init.body), {
		id: "session-1", model: "gpt-6.1-sol", commands: params, max_output_tokens: 10000,
		settings: { allowed_callers: ["direct"], external_web_access: true },
		input: [
			{ type: "message", role: "user", content: [{ type: "input_text", text: "q1" }] },
			{ type: "message", role: "assistant", content: [{ type: "output_text", text: "a1" }] },
			{ type: "message", role: "user", content: [{ type: "input_text", text: "q2" }] },
		],
	});
	await f.tools.get("web_run").execute("call-2", params, undefined, undefined, ctx(DEFAULT));
	assert.deepEqual(f.client.accessCalls, [B, A], "the default provider searches with AuthManager's current account");
	await assert.rejects(f.tools.get("web_run").execute("call-3", params, undefined, undefined, ctx("openai-codex")), /only works with AuthManager/);
	reply = () => new Response("rate limited", { status: 429 });
	await assert.rejects(f.tools.get("web_run").execute("call-4", params, undefined, undefined, ctx(DEFAULT)), /HTTP 429 rate limited/);
	assert.equal(requests.length, 3);
});

test("web_run draws its commands and a short result like pi's own tools", async () => {
	const f = await fixture();
	const tool = f.tools.get("web_run");
	const theme = { fg: (_color: string, text: string) => text, bold: (text: string) => text };
	const draw = (component: any): string => component.render(120).map((line: string) => line.trimEnd()).join("\n").trim();
	assert.equal(draw(tool.renderCall({ search_query: [{ q: "a" }, { q: "b" }], open: [{ ref_id: "turn0search0" }], response_length: "short" }, theme, {})),
		"web_run a · b · open turn0search0");
	const result = { content: [{ type: "text", text: "..." }], details: { results: [{ title: "One", url: "https://example.test/1" }, { title: "Two", url: "https://example.test/2" }] } };
	assert.equal(draw(tool.renderResult(result, { expanded: false, isPartial: false }, theme, { isError: false })), "2 results");
	assert.equal(draw(tool.renderResult(result, { expanded: true, isPartial: false }, theme, { isError: false })), "One https://example.test/1\nTwo https://example.test/2");
	assert.equal(draw(tool.renderResult({ content: [{ type: "text", text: "web_run failed: HTTP 429" }] }, { expanded: false, isPartial: false }, theme, { isError: true })), "web_run failed: HTTP 429");
	// Weather, time and failed opens list no result with a URL; their text is shown without citation marks.
	const weather = { content: [{ type: "text", text: "\ue200cite\ue202turn0forecast0\ue201 Weather for Shanghai:\nCurrent Conditions: Cloudy" }], details: { results: [] } };
	assert.equal(draw(tool.renderResult(weather, { expanded: false, isPartial: false }, theme, { isError: false })), "Weather for Shanghai:");
	assert.equal(draw(tool.renderResult(weather, { expanded: true, isPartial: false }, theme, { isError: false })), "Weather for Shanghai:\nCurrent Conditions: Cloudy");
});
