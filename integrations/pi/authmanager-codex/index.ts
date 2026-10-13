import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { createProvider } from "@earendil-works/pi-ai";
import { builtinProviders } from "@earendil-works/pi-ai/providers/all";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { BrokerClient, RunBinding, type Directory } from "./client.ts";
import { accountSession } from "./session.ts";
import { DEFAULT_PROVIDER as DEFAULT, ProviderNames } from "./names.ts";

const blocked = new Set(["reauth", "attention", "unsaved", "uncertain", "pending"]);

export default async function register(pi: ExtensionAPI): Promise<void> {
	await registerAuthManager(pi, new BrokerClient(), new ProviderNames(fileURLToPath(new URL("./provider-names", import.meta.url))));
}

export async function registerAuthManager(pi: ExtensionAPI, client: BrokerClient, names = new ProviderNames()): Promise<void> {
	const codex = builtinProviders().find((provider) => provider.id === "openai-codex");
	if (!codex) throw new Error("AuthManager 需要 pi 内置的 Codex provider");
	const binding = new RunBinding(client);
	const registered = new Set<string>();
	// pi checks every provider together, and repeatedly. One broker connection per check
	// overflowed the socket's accept queue (ECONNREFUSED → "No API key found"), so checks
	// that overlap share the directory request already in flight.
	let pending: Promise<Directory> | undefined;
	const sharedDirectory = (): Promise<Directory> => pending ??= client.accounts().finally(() => { pending = undefined; });
	let legacy: Record<string, string> = {};
	try {
		const parsed: unknown = JSON.parse(readFileSync(fileURLToPath(new URL("./bindings.json", import.meta.url)), "utf8"));
		if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)
			|| !Object.entries(parsed).every(([key, value]) => /^openai-codex-(personal|luma)$/.test(key) && typeof value === "string" && /^[a-zA-Z0-9-]{1,80}$/.test(value))) throw new Error("invalid");
		legacy = parsed as Record<string, string>;
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw new Error("AuthManager pi 账号关联配置无效");
	}
	const add = (id: string, name: string, accountId?: string, listed = true): void => {
		pi.registerProvider(createProvider({
			id, name, baseUrl: codex.baseUrl,
			auth: { apiKey: {
				name: "AuthManager 管理的 Codex 订阅",
				async check({ signal }) {
					// The installed broker is configured even while its app is restarting.
					// Resolving a request still requires exactly one valid current identity.
					if (!accountId) return { source: "AuthManager（请求时连接）", type: "api_key" as const };
					try {
						signal?.throwIfAborted();
						const directory = await sharedDirectory();
						const account = directory.accounts.find((a) => accountId ? a.id === accountId : a.current);
						return account && !directory.recovery_needed && !blocked.has(account.state) ? { source: "AuthManager", type: "api_key" as const } : undefined;
					} catch { return undefined; }
				},
				async resolve({ signal }) {
					const access = await binding.access(accountId, signal);
					return { auth: { apiKey: access.access_token }, source: `AuthManager · ${access.label}` };
				},
			} },
			models: codex.getModels().map((model) => ({ ...model, provider: id })),
			filterModels: listed ? undefined : () => [],
			api: {
				stream: (model, context, options) => codex.stream(model, context, accountSession(options)),
				streamSimple: (model, context, options) => codex.streamSimple(model, context, accountSession(options)),
			},
		}));
		registered.add(id);
	};
	const update = async (): Promise<Directory> => {
		const directory = await client.accounts();
		const displayIDs = names.resolve(directory.accounts, legacy);
		const wanted = new Set([DEFAULT, ...Object.keys(legacy), ...displayIDs.values(), ...directory.accounts.map((a) => `${DEFAULT}-${a.id}`)]);
		for (const id of registered) if (!wanted.has(id)) { pi.unregisterProvider(id); registered.delete(id); }
		for (const account of directory.accounts) {
			add(displayIDs.get(account.id)!, account.label, account.id);
			add(`${DEFAULT}-${account.id}`, account.label, account.id, false);
		}
		return directory;
	};
	add(DEFAULT, "Codex · 跟随 AuthManager 当前账号");
	for (const [id, accountId] of Object.entries(legacy)) add(id, `Codex · ${id.endsWith("luma") ? "Luma" : "Personal"}（AuthManager）`, accountId);
	// Offline startup must leave unrelated providers usable. Access requests fail explicitly.
	try { await update(); } catch { /* /authmanager reconnects without OAuth login. */ }
	pi.on("before_agent_start", () => { binding.begin(); });
	pi.on("agent_settled", () => { binding.end(); });
	pi.on("session_shutdown", () => { binding.end(); });
	pi.on("session_start", async (_event, ctx) => {
		ctx.ui.setStatus("authmanager", undefined);
		try { await update(); } catch { /* Requests and /authmanager report connection errors. */ }
	});
	pi.registerCommand("authmanager", {
		description: "更新 AuthManager 账号列表；认证与续期在 AuthManager 管理",
		handler: async (_args, ctx) => {
			try {
				const directory = await update();
				const current = directory.accounts.find((a) => a.current)?.label ?? "未选择";
				ctx.ui.notify(`已连接 AuthManager，${directory.accounts.length} 个 Codex 账号；全局当前：${current}。可在 /model 选择跟随当前或指定账号。`, "info");
			} catch (error) { ctx.ui.notify(error instanceof Error ? error.message : "AuthManager 未连接", "error"); }
		},
	});
}
