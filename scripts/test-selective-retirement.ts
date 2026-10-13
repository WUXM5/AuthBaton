// Exact production selective-ui.js with synthetic DOM, fibers, RPC and queries.
// Does not open or connect to a browser, read credentials, or modify repository files.
import fs from 'node:fs';
import {fileURLToPath} from 'node:url';
import vm from 'node:vm';
import assert from 'node:assert/strict';

const sourcePath = process.argv[2] || fileURLToPath(new URL('../macos/Sources/DesktopBridge/selective-ui.js',import.meta.url));
const source = fs.readFileSync(sourcePath, 'utf8');
let identity = 'a', isReady = true, managersAdded = 0, managerConstructions = 0, queryMutations = 0;
function _Sl() {}
function qwl() {}
function gf() {}
function OS() {}
const token = {}, chain = new Map(), node = {token};
const manager = {getConversation() {return null;}};
const client = {
  async cancelQueries() {queryMutations++;},
  async resetQueries() {queryMutations++;},
  getQueryData: () => ({userId: 'b', accountId: 'w'}),
  getQueryCache: () => ({getAll: () => [], subscribe: () => () => {}}),
};
const scope = {
  node, chain, queryClient: client,
  get(key) {
    if (key === refs.Csn) return 'ready';
    if (key === 'UE') return manager;
    if (key === 'WE') return [manager];
    if (key === 'O_') return {};
    return {};
  },
  set() {},
};
node.scope = scope;
const scopeFiber = {
  type: gf, memoizedProps: {scope: token},
  memoizedState: {memoizedState: {current: {node, chain}}},
};
const boundary = {type: _Sl, child: {child: {tag: 7, key: 'nonce:a', child: {type: qwl, child: scopeFiber}}}};
const root = {
  querySelectorAll: () => [], scrollTop: 0, scrollLeft: 0,
  __reactContainer$fixture: {stateNode: {current: {child: boundary}}},
};
const context = vm.createContext({
  Map, Set, Promise, Error, DOMException, setTimeout,
  location: {href: 'app://-/index.html'},
  window: {getSelection: () => null, addEventListener() {}, removeEventListener() {}},
  document: {
    getElementById: () => root, activeElement: null, body: {appendChild() {}},
    createElement: () => ({setAttribute() {}, style: {}, remove() {}}),
  },
});
vm.runInContext(source, context);
const adapter = context.__authManagerSelectiveUIV1;
const jsx = {jsx() {}, jsxs() {}};
const refs = {
  SSl: jsx, _Sl, qwl, gf, $: token, Qd: (_token, _chain, node) => node.scope,
  Up: {identityKey: () => identity, isReady: () => isReady, async refresh() {}},
  Csn: {resolve() {}}, UE: 'UE', O_: 'O_', HE: 'HE', WE: 'WE', i6n() {}, R8n: 'R8n',
  V4n: () => {managerConstructions++; return manager;},
  uD: class {addManager() {managersAdded++;}}, OS,
};
adapter.attach(refs);

(async () => {
  await adapter.begin(true);
  isReady = false;
  const pending = adapter.reconnect('codex:b:w').then(
    () => ({resolved: true}),
    error => ({resolved: false, error: error.message}),
  );
  jsx.jsx(Symbol.for('react.fragment'), {children: {type: qwl}}, 'new-nonce:a');
  const retired = adapter.state();
  const mutationsAtRetirement = queryMutations;
  assert.equal(retired.phase, 'uninstalled');
  identity = 'b';
  isReady = true;
  const outcome = await pending;
  assert.equal(outcome.resolved, false);
  assert.equal(outcome.error, 'ui_adapter_unavailable');
  assert.equal(managerConstructions, 0);
  assert.equal(managersAdded, 0);
  assert.equal(queryMutations - mutationsAtRetirement, 0);
  assert.equal(adapter.state().phase, 'uninstalled');
  // The caller's normal error handler must not revive the retired lease either.
  adapter.failed();
  assert.equal(adapter.state().phase, 'uninstalled');
  console.log(JSON.stringify({
    evidence: 'exact production JS in pure memory; not a browser or desktop test',
    trials: 1, retired, outcome,
    managerConstructionsAfterRetire: managerConstructions,
    managersAddedAfterRetire: managersAdded,
    queryMutationsAfterRetire: queryMutations - mutationsAtRetirement,
    finalState: adapter.state(),
  }, null, 2));
})().catch(error => {console.error(error); process.exitCode = 1;});
