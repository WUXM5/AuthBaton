import { createHash } from "node:crypto";

// pi caches Codex WebSockets by sessionId + workspace. Different users can share
// that workspace, so partition the transport session by the full authenticated identity.
export function accountSession<T extends { sessionId?: string; apiKey?: string }>(options?: T): T | undefined {
	if (!options?.sessionId || !options.apiKey) return options;
	try {
		const claims = JSON.parse(Buffer.from(options.apiKey.split(".")[1], "base64url").toString("utf8"));
		const auth = claims["https://api.openai.com/auth"];
		const user = auth?.chatgpt_user_id ?? auth?.user_id;
		const workspace = auth?.chatgpt_account_id;
		if (typeof user !== "string" || !user || typeof workspace !== "string" || !workspace) throw new Error();
		const identity = createHash("sha256").update(`${user}\0${workspace}`).digest("hex").slice(0, 24);
		return { ...options, sessionId: `${options.sessionId}-am-${identity}` };
	} catch { throw new Error("AuthManager 返回的 Codex 身份无效"); }
}
