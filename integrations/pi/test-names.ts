import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { Account } from "./authmanager-codex/client.ts";
import { ProviderNames } from "./authmanager-codex/names.ts";

const account = (id: string, label: string): Account => ({ id, label, current: false, state: "healthy", automatic: false });

test("readable provider names use labels and the legacy personal alias", () => {
	const names = new ProviderNames().resolve([
		account("id-a", "Codex · nova"), account("id-b", "Codex · orbit"),
		account("id-c", "lab team"), account("id-d", "Codex 当前账号"), account("id-e", "person@example.test"),
	], { "openai-codex-personal": "id-d" });
	assert.deepEqual([...names.values()], ["authmanager-codex-nova", "authmanager-codex-orbit", "authmanager-codex-lab-team", "authmanager-codex-personal", "authmanager-codex-person@example.test"]);
});

test("duplicate names never replace another account and never collide with old UUID aliases", () => {
	const names = new ProviderNames().resolve([account("nova", "nova"), account("b", "nova")]);
	assert.notEqual(names.get("nova"), names.get("b"));
	assert.notEqual(names.get("nova"), "authmanager-codex-nova");
	assert.notEqual(names.get("b"), "authmanager-codex-nova");
});

test("names survive reload and rename; deleted names stay bound to their original owner", () => {
	const root = mkdtempSync(join(tmpdir(), "authmanager-pi-names-"));
	try {
		const first = new ProviderNames(root).resolve([account("id-a", "Codex · nova")]);
		const second = new ProviderNames(root).resolve([account("id-a", "new email@example.test"), account("id-b", "nova")]);
		assert.equal(second.get("id-a"), first.get("id-a"));
		assert.equal(second.get("id-b"), "authmanager-codex-nova-2");
		const third = new ProviderNames(root).resolve([account("id-c", "nova")]);
		assert.equal(third.get("id-c"), "authmanager-codex-nova-3");
		assert.equal(JSON.parse(readFileSync(join(root, "authmanager-codex-nova.json"), "utf8")), "id-a");
		assert.equal(readdirSync(root).some((name) => name.endsWith(".tmp")), false);
	} finally { rmSync(root, { recursive: true, force: true }); }
});

test("invalid bindings stop allocation instead of remapping a saved provider", () => {
	const root = mkdtempSync(join(tmpdir(), "authmanager-pi-names-"));
	try {
		writeFileSync(join(root, "authmanager-codex-nova.json"), "{}");
		assert.throws(() => new ProviderNames(root).resolve([account("id-a", "nova")]), /名称关联无效/);
		assert.deepEqual(readdirSync(root), ["authmanager-codex-nova.json"]);
	} finally { rmSync(root, { recursive: true, force: true }); }
});
