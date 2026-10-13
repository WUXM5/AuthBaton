import { randomUUID } from "node:crypto";
import { linkSync, mkdirSync, readFileSync, readdirSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Account } from "./client.ts";

export const DEFAULT_PROVIDER = "authmanager-codex";
const validName = /^authmanager-codex-[\p{L}\p{N}@._+-]{1,80}$/u;
const validAccount = /^[a-zA-Z0-9-]{1,80}$/;

// Display IDs are permanent bindings: renaming/deleting an account must not
// silently route a saved pi model selection to somebody else's credentials.
export class ProviderNames {
	private readonly bindings = new Map<string, string>();
	private readonly directory?: string;
	constructor(directory?: string) { this.directory = directory; }
	private load(): void {
		if (!this.directory) return;
		mkdirSync(this.directory, { recursive: true, mode: 0o700 });
		for (const file of readdirSync(this.directory).sort()) {
			if (!file.endsWith(".json")) continue;
			const name = file.slice(0, -5);
			const id: unknown = JSON.parse(readFileSync(join(this.directory, file), "utf8"));
			if (!validName.test(name) || typeof id !== "string" || !validAccount.test(id)) {
				throw new Error("AuthManager pi 账号名称关联无效，未更改已有绑定");
			}
			this.bindings.set(name, id);
		}
	}
	private claim(name: string, id: string): boolean {
		if (this.bindings.has(name)) return this.bindings.get(name) === id;
		if (this.directory) {
			const path = join(this.directory, `${name}.json`);
			const temporary = join(this.directory, `.${randomUUID()}.tmp`);
			writeFileSync(temporary, `${JSON.stringify(id)}\n`, { mode: 0o600, flag: "wx" });
			try {
				// Publish complete metadata without replacing another pi process's claim.
				try { linkSync(temporary, path); }
				catch (error) {
					if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
					const owner: unknown = JSON.parse(readFileSync(path, "utf8"));
					if (typeof owner !== "string" || !validAccount.test(owner)) throw new Error("AuthManager pi 账号名称关联无效");
					this.bindings.set(name, owner);
					return owner === id;
				}
			} finally { unlinkSync(temporary); }
		}
		this.bindings.set(name, id);
		return true;
	}
	resolve(accounts: Account[], legacy: Record<string, string> = {}): Map<string, string> {
		this.load();
		const result = new Map<string, string>();
		const reserved = new Set(accounts.map((a) => `${DEFAULT_PROVIDER}-${a.id}`));
		for (const account of [...accounts].sort((a, b) => a.id.localeCompare(b.id))) {
			const existing = [...this.bindings].find(([, id]) => id === account.id)?.[0];
			if (existing) { result.set(account.id, existing); continue; }
			const legacyName = Object.entries(legacy).find(([, id]) => id === account.id)?.[0].replace(/^openai-codex-/, "");
			const slug = (legacyName ?? account.label).normalize("NFKC").replace(/^codex\s*(?:[·:：-]\s*)?/iu, "")
				.trim().toLowerCase().replace(/[^\p{L}\p{N}@._+-]+/gu, "-").replace(/^-+|-+$/g, "").slice(0, 48) || "account";
			const base = `${DEFAULT_PROVIDER}-${slug}`;
			for (let suffix = 1; ; suffix++) {
				const name = suffix === 1 ? base : `${base}-${suffix}`;
				if (!reserved.has(name) && this.claim(name, account.id)) { result.set(account.id, name); break; }
			}
		}
		return result;
	}
}
