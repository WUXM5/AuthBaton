// Run with Node; reads only the already-extracted official JavaScript bundle.
// The Qst mount and account-input effect bodies are exact source. RPC and mount ownership are
// synthetic; this does not instantiate a desktop, read credentials or use network.
import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';

const sourcePath = process.argv[2];
if (!sourcePath) {
  console.error('Usage: node --experimental-strip-types scripts/test-selective-lifecycle.ts <bundle>\n'
    + '  <bundle>: webview/assets/app-shared-*.js extracted from the official Codex desktop app');
  process.exit(2);
}
const source = fs.readFileSync(sourcePath, 'utf8');
// Qst and the $st/ect disposal helpers that follow it.
const mountStart = source.indexOf('function Qst(');
const mountSource = source.slice(mountStart, source.indexOf('function tct(', mountStart));
const effectStart = source.indexOf('tq=Wk(') + 'tq='.length;
const effectSource = source.slice(effectStart, source.indexOf(',nq=', effectStart));
assert(mountStart >= 0 && effectStart > 4 && mountSource.includes('s.onMount='));
assert(effectSource.includes('Account input subscription unavailable'));

function fixture() {
  const subscriptions = [];
  let state, watchers = 0, resets = 0;
  const scope = {get: () => 0, queryClient: {
    getQueryCache: () => ({findAll: () => [{reset() {resets++;}, setState() {}}]}),
    resetQueries() {}, invalidateQueries() {},
  }};
  const node = {imperativeReadDepth: 0, store: {set(_atom, value) {state = value;}}};
  const context = vm.createContext({
    X: {}, wAn: {}, eq: [['synthetic-account']], ZK: ['synthetic'], JO: {batch: fn => fn()},
    e6: {accessInputs: {subscribe(listener) {
      const item = {active: true, listener, onBroken: null};
      subscriptions.push(item);
      return {
        onRpcBroken(fn) {item.onBroken = fn;},
        then(fn) {fn({[Symbol.dispose]() {item.active = false;}});},
        [Symbol.dispose]() {item.active = false;},
      };
    }}},
    dst: () => ({}), ak() {}, Fk: () => ({scope}),
    Nk: () => ({watch(fn) {
      watchers++;
      const cleanup = fn(scope);
      return () => {watchers--; cleanup?.();};
    }}),
    globalThis: {dispatchEvent() {}}, ErrorEvent: class {},
  });
  vm.runInContext(mountSource, context);
  context.Wk = (token, initial, effect) => context.Qst(token, node, {}, e => typeof initial === 'function' ? initial(e) : initial, effect);
  const atom = vm.runInContext(effectSource, context);
  const set = value => {state = value;};
  let consumers = 0, managedCleanup;
  function subscribe() {
    if (consumers++ === 0) managedCleanup = atom.onMount(set);
    let active = true;
    return () => {
      if (!active) return;
      active = false;
      if (--consumers === 0) {managedCleanup?.(); managedCleanup = undefined;}
    };
  }
  return {
    mountManually: () => atom.onMount(set), subscribe,
    breakLatest() {
      const subscription = subscriptions.at(-1);
      subscription.active = false;
      subscription.onBroken(new Error('synthetic disconnected'));
    },
    state: () => ({state, activeSubscriptions: subscriptions.filter(s => s.active).length, watchers, resets, consumers}),
  };
}

const negative = fixture();
let ui = negative.subscribe();
negative.breakLatest();
assert.equal(negative.state().state, 'error');
const manual = negative.mountManually();
assert.equal(negative.state().activeSubscriptions, 1);
ui();
ui = negative.subscribe();
const negativeResult = negative.state();
assert.equal(negativeResult.activeSubscriptions, 2);
manual(); ui();
assert.equal(negative.state().activeSubscriptions, 0);

const retained = fixture();
let ui2 = retained.subscribe();
retained.breakLatest();
const releaseRetain = retained.subscribe();
const fallback = retained.mountManually();
ui2();
ui2 = retained.subscribe();
const retainedResult = retained.state();
assert.equal(retainedResult.activeSubscriptions, 1);
assert.equal(retainedResult.consumers, 2);
fallback(); releaseRetain(); ui2();
assert.equal(retained.state().activeSubscriptions, 0);

console.log(JSON.stringify({
  evidence: 'exact official Qst and account-input effect bodies; synthetic RPC and consumer lifecycle',
  negative: negativeResult,
  retained: retainedResult,
  finalActive: [negative.state().activeSubscriptions, retained.state().activeSubscriptions],
}, null, 2));
