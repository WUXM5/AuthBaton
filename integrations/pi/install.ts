// Explicit, local-only migration. Credentials are never printed or sent to a network.
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { DatabaseSync } from "node:sqlite";
import { chmod, copyFile, mkdir, readFile, rename, stat, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { BrokerClient } from "./authmanager-codex/client.ts";

const home = homedir();
const root = join(home, ".pi/agent");
const extension = join(root, "extensions/authmanager-codex");
const authPath = join(root, "auth.json");
const oldPath = join(root, "extensions/codex-accounts.ts");
const source = join(dirname(fileURLToPath(import.meta.url)), "authmanager-codex");
const legacyKeys = ["openai-codex-personal", "openai-codex-luma"];
const globalModules = execFileSync("npm", ["root", "-g"], { encoding: "utf8" }).trim();
const require = createRequire(join(globalModules, "@earendil-works/pi-coding-agent/package.json"));
const lockfile = require("proper-lockfile");
const directory = await new BrokerClient().accounts();
const db = new DatabaseSync(join(home, "Library/Application Support/AuthManager/accounts.sqlite3"), { readOnly: true });
let managed: any[];
try { managed = JSON.parse((db.prepare("SELECT body FROM state WHERE id=1").get() as { body: string }).body).accounts; }
finally { db.close(); }
let previousBindings: Record<string, string> = {};
try { previousBindings = JSON.parse(await readFile(join(extension, "bindings.json"), "utf8")); }
catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw new Error("已有账号关联文件不可读，未迁移"); }

const release = await lockfile.lock(authPath, { realpath: false, stale: 30_000, retries: { retries: 5, minTimeout: 100, maxTimeout: 500 } });
try {
	const original = await readFile(authPath);
	const auth = JSON.parse(original.toString("utf8"));
	if (typeof auth !== "object" || auth === null || Array.isArray(auth)) throw new Error("pi 认证文件格式无效，未迁移");
	const bindings: Record<string, string> = { ...previousBindings };
	for (const key of legacyKeys) {
		const credential = auth[key];
		if (!credential) continue;
		if (credential.type !== "oauth" || typeof credential.access !== "string") throw new Error(`${key} 不是预期的 OAuth 配置，未迁移`);
		let claims: any;
		try { claims = JSON.parse(Buffer.from(credential.access.split(".")[1], "base64url").toString()); }
		catch { throw new Error(`${key} 身份无法读取，未迁移`); }
		const identity = claims["https://api.openai.com/auth"];
		if (!identity?.chatgpt_account_id || identity.chatgpt_account_id !== credential.accountId) throw new Error(`${key} 工作区身份不一致，未迁移`);
		const owner = `codex:${identity?.chatgpt_user_id ?? identity?.user_id}:${credential.accountId}`;
		const matches = managed.filter((a) => a.provider === "codex" && a.metadata?.owner === owner);
		if (matches.length !== 1 || !directory.accounts.some((a) => a.id === matches[0].id)) throw new Error(`${key} 未在 AuthManager 唯一登记，未迁移`);
		bindings[key] = matches[0].id;
	}
	if (Object.values(bindings).some((id) => !directory.accounts.some((a) => a.id === id))) throw new Error("关联账号已移除，未迁移");
	if (!process.argv.includes("--install")) {
		console.log(JSON.stringify({ mode: "preview", managedAccounts: directory.accounts.length, aliases: Object.keys(bindings), changes: "安装 authmanager-codex；备份后移除两条 pi 独立 OAuth；保留其他凭据与默认模型" }));
	} else {
		const backup = join(root, "backups", `authmanager-codex-${new Date().toISOString().replaceAll(":", "-")}`);
		await mkdir(backup, { recursive: true, mode: 0o700 });
		await writeFile(join(backup, "auth.json"), original, { mode: 0o600, flag: "wx" });
		await mkdir(extension, { recursive: true, mode: 0o700 });
		for (const file of ["index.ts", "client.ts", "session.ts", "names.ts", "web-run.ts", "bindings.json"]) {
			try { await copyFile(join(extension, file), join(backup, file)); }
			catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
		}
		// Prepare the replacement before retiring the old extension or credential entries.
		for (const file of ["index.ts", "client.ts", "session.ts", "names.ts", "web-run.ts"]) await copyFile(join(source, file), join(extension, file));
		await writeFile(join(extension, "bindings.json"), `${JSON.stringify(bindings, null, 2)}\n`, { mode: 0o600 });
		for (const key of legacyKeys) delete auth[key];
		const temporary = `${authPath}.authmanager-${process.pid}.tmp`;
		await writeFile(temporary, `${JSON.stringify(auth, null, 2)}\n`, { mode: (await stat(authPath)).mode & 0o777, flag: "wx" });
		try { await rename(oldPath, join(backup, "codex-accounts.ts")); }
		catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
		await rename(temporary, authPath);
		await chmod(join(backup, "auth.json"), 0o600);
		console.log(JSON.stringify({ installed: extension, backup, aliases: Object.keys(bindings), managedAccounts: directory.accounts.length, defaultModelUnchanged: true }));
	}
} finally { await release(); }
