// Exact production selective-ui.js with synthetic DOM, fibers, RPC and queries.
// Covers the 26.924 reconnect race (the reset's fetch still runs stale options), open tasks across the
// switch (26.1007 opens the page it remembers for the new account) and revision upgrades.
// Does not open or connect to a browser, read credentials, or modify repository files.
import fs from 'node:fs';
import {fileURLToPath} from 'node:url';
import vm from 'node:vm';
import assert from 'node:assert/strict';

const source = fs.readFileSync(fileURLToPath(new URL('../macos/Sources/DesktopBridge/selective-ui.js', import.meta.url)), 'utf8');

function fixture(existing?: object) {
  let identity = 'a', data: any = {userId: 'a', accountId: 'w'}, refetches = 0, answerOnRefetch = true;
  const created: string[] = [], added: unknown[] = [];
  // Open task routes, and the tasks each manager has resumed. The new manager starts empty.
  let routes: object[] = [], resumedBefore = new Set<string>(), resumeAfter: Record<string, number> = {}, polls = 0;
  // Capture listeners the adapter adds to block input, page reloads it asks for, active refetches.
  const listening = new Map<string, number>();
  let reloads = 0, activeRefetches = 0;
  const next = {getConversation(id: string) {
    polls++;
    return id in resumeAfter && polls > resumeAfter[id] ? {resumeState: 'resumed'} : null;
  }};
  function _Sl() {}
  function qwl() {}
  function gf() {}
  function OS() {}
  const token = {}, chain = new Map(), node: any = {token};
  const manager = {getConversation: (id: string) => resumedBefore.has(id) ? {resumeState: 'resumed'} : null};
  const query = {state: {fetchStatus: 'idle'}};
  const client = {
    async cancelQueries() {},
    // The fetch started by the reset runs options computed while account inputs were broken: no data.
    async resetQueries() {data = undefined;},
    async refetchQueries({queryKey, type}: {queryKey?: string[], type?: string}) {
      if (type === 'active' && !queryKey) {activeRefetches++; return;}
      // The key is built inside the vm realm, so compare by value.
      if (JSON.stringify(queryKey) !== JSON.stringify(['vscode', 'account-info'])) throw Error('unexpected query key');
      refetches++;
      if (answerOnRefetch) data = {userId: identity, accountId: 'w'};
    },
    getQueryData: () => data,
    getQueryCache: () => ({getAll: () => [], subscribe: () => () => {}, find: () => query}),
  };
  const scope = {node, chain, queryClient: client, get: (key: unknown) => key === refs.Csn ? 'ready' : {}, set() {}};
  node.scope = scope;
  const scopeFiber: any = {type: gf, memoizedProps: {scope: token}, memoizedState: {memoizedState: {current: {node, chain}}},
    get child() {return routes.reduceRight((sibling: any, value) => ({type: gf, memoizedProps: {scope: OS, value}, sibling}), null);}};
  const boundary = {type: _Sl, child: {child: {tag: 7, key: 'nonce:a', child: {type: qwl, child: scopeFiber}}}};
  const root = {querySelectorAll: () => [], scrollTop: 0, scrollLeft: 0, __reactContainer$fixture: {stateNode: {current: {child: boundary}}}};
  const context = vm.createContext({
    Map, Set, Promise, Error, DOMException, setTimeout: (run: () => void) => setImmediate(run),
    location: {href: 'app://-/index.html', reload() {reloads++;}},
    window: {getSelection: () => null,
             addEventListener(type: string) {listening.set(type, (listening.get(type) ?? 0) + 1);},
             removeEventListener(type: string) {listening.set(type, (listening.get(type) ?? 0) - 1);}},
    document: {getElementById: () => root, activeElement: null, body: {appendChild() {}}, createElement: () => ({setAttribute() {}, style: {}, remove() {}})},
  });
  if (existing) context.__authManagerSelectiveUIV1 = existing;
  const installed = vm.runInContext(source, context);
  const jsx = {jsx() {}, jsxs() {}};
  const refs = {
    SSl: jsx, _Sl, qwl, gf, $: token, Qd: (_t: unknown, _c: unknown, n: any) => n.scope,
    Up: {identityKey: () => identity, isReady: () => true, async refresh() {}},
    Csn: {resolve() {}}, O_: 'O_',
    // refreshRendererManager deletes first; the adapter must never call it.
    X3: class {getForHostId() {return added.at(-1) ?? manager;} addManager(m: unknown) {added.push(m);} refreshRendererManager() {throw Error('delete-first refresh');}},
    loi: (_scope: unknown, host: string) => {created.push(host); return next;}, OS,
  };
  const adapter = context.__authManagerSelectiveUIV1;
  return {installed, adapter, refs, attach: () => adapter.attach(refs), switchTo: (next: string) => {identity = next;},
          refetches: () => refetches, created: () => [...created], added: () => [...added], next, noAnswer: () => {answerOnRefetch = false;},
          // Opens a task the old manager had resumed; the new one resumes it after `polls` reads, or never.
          openTask: (id: string, polls?: number) => {routes = [{routeKind: 'local-thread', conversationId: id}]; resumedBefore.add(id); if (polls !== undefined) resumeAfter[id] = polls;},
          goHome: () => {routes = [{routeKind: 'home'}];},
          blocked: () => (listening.get('keydown') ?? 0) > 0, reloads: () => reloads, activeRefetches: () => activeRefetches};
}

let passed = 0;
async function test(name: string, run: () => Promise<void>): Promise<void> {await run(); passed++; console.log(`PASS ${name}`);}

await test('reconnect asks for account info again when the reset fetch ran stale options', async () => {
  const f = fixture();
  f.attach();
  await f.adapter.begin(true);
  f.switchTo('b');
  const result = await f.adapter.reconnect('codex:b:w');
  assert.equal(result.ok, true);
  assert.equal(f.refetches(), 1);
  assert.deepEqual(f.created(), ['local']);
  assert.equal(f.added().length, 1);
  assert.equal(f.added()[0], f.next);
  assert.equal(f.adapter.state().phase, 'idle');
});

await test('a host registry without addManager or a manager factory is refused', async () => {
  const f = fixture();
  f.refs.X3 = class {getForHostId() {return {};}};
  assert.throws(() => f.attach(), /ui_adapter_unavailable/);
  assert.equal(f.adapter.state().phase, 'uninstalled');
  const g = fixture();
  delete g.refs.loi;
  assert.throws(() => g.attach(), /ui_adapter_unavailable/);
});

await test('reconnect still fails, after its deadline, when account info never arrives', async () => {
  const f = fixture();
  f.attach();
  f.noAnswer();
  await f.adapter.begin(true);
  f.switchTo('b');
  await assert.rejects(f.adapter.reconnect('codex:b:w'), /ui_services_unavailable/);
  assert.equal(f.refetches(), 37);
});

await test('a task the page left during the switch is not awaited', async () => {
  // 13:49 on 2026-10-14: Codex opened the home page it remembered for the new account, the task never resumed
  // and the switch failed after 15 seconds although the new account was in place.
  const f = fixture();
  f.attach();
  f.openTask('task-1');
  await f.adapter.begin(true);
  f.switchTo('b');
  f.goHome();
  const result = await f.adapter.reconnect('codex:b:w');
  assert.equal(result.ok, true);
  assert.equal(result.contextPreserved, false);
  assert.equal(result.threadsResumed, undefined);
  assert.equal(f.adapter.state().phase, 'idle');
});

await test('a task still open is awaited until the new manager resumes it', async () => {
  const f = fixture();
  f.attach();
  f.openTask('task-1', 20);
  await f.adapter.begin(true);
  f.switchTo('b');
  const result = await f.adapter.reconnect('codex:b:w');
  assert.equal(result.ok, true);
  assert.equal(result.contextPreserved, true);
  assert.equal(result.threadsResumed, undefined);
});

await test('a task that stays open and never resumes releases the page instead of holding it', async () => {
  const f = fixture();
  f.attach();
  f.openTask('task-1');
  await f.adapter.begin(true);
  f.switchTo('b');
  const result = await f.adapter.reconnect('codex:b:w');
  assert.equal(result.ok, true);
  assert.equal(result.contextPreserved, false);
  assert.equal(result.threadsResumed, false);
  assert.equal(f.adapter.state().phase, 'idle');
});

await test('an idle older adapter is disposed and replaced', async () => {
  let disposed = 0;
  const f = fixture({revision: 3, state: () => ({phase: 'idle'}), dispose() {disposed++;}});
  assert.equal(disposed, 1);
  assert.equal(f.adapter.revision, 6);
});

await test('an older adapter with a switch in flight is kept until it finishes', async () => {
  let disposed = 0;
  const old = {revision: 5, state: () => ({phase: 'reconnecting'}), dispose() {disposed++;}};
  const f = fixture(old);
  assert.equal(f.installed, true);
  assert.equal(disposed, 0);
  assert.equal(f.adapter, old);
  assert.equal(f.reloads(), 0);
});

await test('an older adapter still holding the page after a failed switch is released by a reload', async () => {
  // Revision 5 kept the page blocked in `failed` until a recovery from AuthManager, which no longer exists.
  let disposed = 0;
  const old = {revision: 5, state: () => ({phase: 'failed'}), dispose() {disposed++;}};
  const f = fixture(old);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(disposed, 0);
  assert.equal(f.reloads(), 1);
});

await test('a switch that fails before the login lands gives the page back as it was', async () => {
  const f = fixture();
  f.attach();
  await f.adapter.begin(true);
  assert.equal(f.blocked(), true);
  // The bridge reports a login that never landed: the app-server and the page still hold account a.
  assert.equal(f.adapter.failed(false).reloading, false);
  assert.equal(f.adapter.state().phase, 'idle');
  assert.equal(f.blocked(), false);
  assert.equal(f.activeRefetches(), 1);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(f.reloads(), 0);
  assert.deepEqual(f.added(), []);
  // Nothing is held: the next switch starts normally.
  await f.adapter.begin(true);
  f.switchTo('b');
  assert.equal((await f.adapter.reconnect('codex:b:w')).ok, true);
});

await test('a switch that fails after the login landed hands the page to Codex reset by reloading', async () => {
  const f = fixture();
  f.attach();
  f.noAnswer();
  await f.adapter.begin(true);
  f.switchTo('b');
  await assert.rejects(f.adapter.reconnect('codex:b:w'), /ui_services_unavailable/);
  assert.equal(f.blocked(), true);
  assert.equal(f.adapter.failed(true).reloading, true);
  assert.equal(f.adapter.state().phase, 'uninstalled');
  assert.equal(f.blocked(), false);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(f.reloads(), 1);
});

await test('tokens applied while Codex reports the login failed: the page is rebuilt although its account never changed', async () => {
  // Real lab, 2026-10-14: login/completed success:false (workspace routing discovery failed), no
  // account/updated, the app-server already on the new account and the page stuck on the old one.
  const f = fixture();
  f.attach();
  await f.adapter.begin(true);
  assert.equal(f.adapter.failed(true).reloading, true);
  assert.equal(f.blocked(), false);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(f.reloads(), 1);
});

await test('an unknown outcome is treated as a changed account', async () => {
  const f = fixture();
  f.attach();
  await f.adapter.begin(true);
  assert.equal(f.adapter.failed().reloading, true);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(f.reloads(), 1);
});

console.log(JSON.stringify({passed, evidence: 'exact production JS in pure memory; not a browser or desktop test'}));
