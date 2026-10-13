import { randomUUID } from "node:crypto";
import { lstat } from "node:fs/promises";
import { createConnection } from "node:net";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

export interface Account { id: string; label: string; current: boolean; state: string; automatic: boolean }
export interface Directory { accounts: Account[]; recovery_needed: boolean }
export interface Access { state: "ready"; account_id: string; label: string; access_token: string; expires_at: number }
type Waiting = { state: "waiting"; account_id: string; retry_after_ms: number };
const detail = (code?: string): string => code ? `（${code}）` : "";
const unavailable = (code?: string): string => `无法连接 AuthManager${detail(code)}，请打开支持 pi 接入的版本并保持菜单栏运行`;
const dropped = (code?: string): string => `AuthManager 未返回结果就断开了连接${detail(code)}，请重试`;
const unsafe = "AuthManager 的 pi socket 权限或所有者异常，已拒绝连接";
const invalid = "AuthManager 返回了无效的认证响应";
const record = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);

export class BrokerClient {
	readonly path: string;
	constructor(path = join(homedir(), "Library/Application Support/AuthManager/pi.sock")) { this.path = path; }
	async rpc(method: "accounts" | "access", accountId?: string, signal?: AbortSignal): Promise<unknown> {
		signal?.throwIfAborted();
		const [socket, parent] = await Promise.all([lstat(this.path), lstat(dirname(this.path))])
			.catch((error: NodeJS.ErrnoException) => { throw new Error(unavailable(error.code)); });
		if (!socket.isSocket() || !parent.isDirectory() || (socket.mode & 0o077) !== 0 || (parent.mode & 0o077) !== 0
			|| socket.uid !== process.getuid?.() || parent.uid !== process.getuid?.()) throw new Error(unsafe);
		const id = randomUUID();
		return new Promise((resolve, reject) => {
			const socket = createConnection({ path: this.path });
			let buffer = Buffer.alloc(0);
			let settled = false;
			let connected = false;
			const finish = (error?: Error, result?: unknown): void => {
				if (settled) return;
				settled = true;
				signal?.removeEventListener("abort", abort);
				socket.destroy();
				if (error) reject(error); else resolve(result);
			};
			const abort = (): void => finish(new Error("AuthManager 认证请求已取消"));
			signal?.addEventListener("abort", abort, { once: true });
			if (signal?.aborted) { abort(); return; }
			socket.setTimeout(8_000, () => finish(new Error("AuthManager 暂未响应，请稍后重试")));
			socket.once("error", (error: NodeJS.ErrnoException) => finish(new Error((connected ? dropped : unavailable)(error.code))));
			socket.once("end", () => finish(new Error(dropped())));
			socket.once("connect", () => {
				connected = true;
				socket.write(`${JSON.stringify({ version: 1, id, method, ...(accountId ? { account_id: accountId } : {}) })}\n`);
			});
			socket.on("data", (chunk: Buffer) => {
				buffer = Buffer.concat([buffer, chunk]);
				if (buffer.length > 65_536) { finish(new Error(invalid)); return; }
				const end = buffer.indexOf(10);
				if (end < 0) return;
				let envelope: unknown;
				try { envelope = JSON.parse(buffer.subarray(0, end).toString("utf8")); }
				catch { finish(new Error(invalid)); return; }
				if (!record(envelope) || envelope.version !== 1 || envelope.id !== id) { finish(new Error(invalid)); return; }
				if (typeof envelope.error === "string") { finish(new Error(envelope.error)); return; }
				finish(undefined, envelope.result);
			});
		});
	}
	async accounts(signal?: AbortSignal): Promise<Directory> {
		const value = await this.rpc("accounts", undefined, signal);
		if (!record(value) || typeof value.recovery_needed !== "boolean" || !Array.isArray(value.accounts)
			|| !value.accounts.every((a: unknown) => record(a) && typeof a.id === "string" && /^[a-zA-Z0-9-]{1,80}$/.test(a.id)
				&& typeof a.label === "string" && typeof a.current === "boolean" && typeof a.state === "string" && typeof a.automatic === "boolean")) throw new Error(invalid);
		return value as unknown as Directory;
	}
	async access(accountId: string, signal?: AbortSignal): Promise<Access> {
		const deadline = Date.now() + 90_000;
		while (true) {
			const value = await this.rpc("access", accountId, signal);
			if (!record(value) || value.account_id !== accountId) throw new Error(invalid);
			if (value.state === "ready") {
				if (typeof value.access_token !== "string" || !value.access_token || /[\x00-\x20\x7f]/.test(value.access_token)
					|| typeof value.label !== "string" || typeof value.expires_at !== "number" || !Number.isSafeInteger(value.expires_at)
					|| value.expires_at * 1000 <= Date.now()) throw new Error(invalid);
				return value as unknown as Access;
			}
			if (value.state !== "waiting" || typeof value.retry_after_ms !== "number") throw new Error(invalid);
			if (Date.now() >= deadline) throw new Error("AuthManager 仍在处理续期，请稍后重试；未切换到其他账号");
			await delay(Math.min(2_000, Math.max(250, (value as unknown as Waiting).retry_after_ms)), undefined, { signal });
		}
	}
}

/** Pin one identity through automatic retries/compaction until agent_settled. Never cache tokens. */
export class RunBinding {
	private run?: { selected?: Promise<string> };
	readonly client: BrokerClient;
	constructor(client: BrokerClient) { this.client = client; }
	begin(): void { this.run ??= {}; }
	end(): void { this.run = undefined; }
	private async select(requested?: string, signal?: AbortSignal): Promise<string> {
		const directory = await this.client.accounts(signal);
		if (directory.recovery_needed) throw new Error("请先在 AuthManager 恢复未完成的账号切换");
		const accounts = directory.accounts.filter((a) => requested ? a.id === requested : a.current);
		if (accounts.length !== 1) throw new Error(requested ? "此账号已移除，请在 pi 运行 /authmanager 更新列表" : "请在 AuthManager 选择唯一的全局 Codex 当前账号");
		return accounts[0].id;
	}
	async access(requested?: string, signal?: AbortSignal): Promise<Access> {
		const run = this.run;
		const id = run ? await (run.selected ??= this.select(requested, signal)) : await this.select(requested, signal);
		if (requested && requested !== id) throw new Error("当前任务已固定账号，请等任务结束后再更换账号");
		return this.client.access(id, signal);
	}
}
