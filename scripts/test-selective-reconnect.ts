// Exact production selective-ui.js with synthetic DOM, fibers, RPC and queries.
// Covers the 26.924 reconnect race (the reset's fetch still runs stale options) and revision upgrades.
// Does not open or connect to a browser, read credentials, or modify repository files.
import fs from 'node:fs';
import {fileURLToPath} from 'node:url';
import vm from 'node:vm';
import assert from 'node:assert/strict';

const source = fs.readFileSync(fileURLToPath(new URL('../macos/Sources/DesktopBridge/selective-ui.js', import.meta.url)), 'utf8');

function fixture(existing?: object) {
  let identity = 'a', data: any = {userId: 'a', accountId: 'w'}, refetches = 0, answerOnRefetch = true;
  const refreshed: string[] = [];
  function _Sl() {}
  function qwl() {}
  function gf() {}
  function OS() {}
  const token = {}, chain = new Map(), node: any = {token};
  const manager = {getConversation() {return null;}};
  const query = {state: {fetchStatus: 'idle'}};
  const client = {
    async cancelQueries() {},
    // The fetch started by the reset runs options computed while account inputs were broken: no data.
    async resetQueries() {data = undefined;},
    async refetchQueries({queryKey}: {queryKey: string[]}) {
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
  const scopeFiber = {type: gf, memoizedProps: {scope: token}, memoizedState: {memoizedState: {current: {node, chain}}}};
  const boundary = {type: _Sl, child: {child: {tag: 7, key: 'nonce:a', child: {type: qwl, child: scopeFiber}}}};
  const root = {querySelectorAll: () => [], scrollTop: 0, scrollLeft: 0, __reactContainer$fixture: {stateNode: {current: {child: boundary}}}};
  const context = vm.createContext({
    Map, Set, Promise, Error, DOMException, setTimeout: (run: () => void) => setImmediate(run),
    location: {href: 'app://-/index.html'},
    window: {getSelection: () => null, addEventListener() {}, removeEventListener() {}},
    document: {getElementById: () => root, activeElement: null, body: {appendChild() {}}, createElement: () => ({setAttribute() {}, style: {}, remove() {}})},
  });
  if (existing) context.__authManagerSelectiveUIV1 = existing;
  const installed = vm.runInContext(source, context);
  const jsx = {jsx() {}, jsxs() {}};
  const refs = {
    SSl: jsx, _Sl, qwl, gf, $: token, Qd: (_t: unknown, _c: unknown, n: any) => n.scope,
    Up: {identityKey: () => identity, isReady: () => true, async refresh() {}},
    Csn: {resolve() {}}, O_: 'O_',
    X3: class {getForHostId() {return manager;} refreshRendererManager(host: string) {refreshed.push(host);}}, OS,
  };
  const adapter = context.__authManagerSelectiveUIV1;
  return {installed, adapter, refs, attach: () => adapter.attach(refs), switchTo: (next: string) => {identity = next;},
          refetches: () => refetches, refreshed: () => [...refreshed], noAnswer: () => {answerOnRefetch = false;}};
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
  assert.deepEqual(f.refreshed(), ['local']);
  assert.equal(f.adapter.state().phase, 'idle');
});

await test('a host registry without the official refresh is refused', async () => {
  const f = fixture();
  f.refs.X3 = class {getForHostId() {return {};}};
  assert.throws(() => f.attach(), /ui_adapter_unavailable/);
  assert.equal(f.adapter.state().phase, 'uninstalled');
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

await test('an idle older adapter is disposed and replaced', async () => {
  let disposed = 0;
  const f = fixture({revision: 2, state: () => ({phase: 'idle'}), dispose() {disposed++;}});
  assert.equal(disposed, 1);
  assert.equal(f.adapter.revision, 3);
});

await test('an older adapter with an unfinished switch is kept for its recovery', async () => {
  let disposed = 0;
  const old = {revision: 2, state: () => ({phase: 'failed'}), dispose() {disposed++;}};
  const f = fixture(old);
  assert.equal(f.installed, true);
  assert.equal(disposed, 0);
  assert.equal(f.adapter, old);
});

console.log(JSON.stringify({passed, evidence: 'exact production JS in pure memory; not a browser or desktop test'}));
