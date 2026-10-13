// Private renderer adapter with structural checks. No credentials, disk writes or navigation.
(() => {
  const key = '__authManagerSelectiveUIV1';
  const existing = globalThis[key];
  if (existing?.revision === 2) return true;
  // A switch is finished or recovered by the adapter that started it; an idle older one is replaced.
  if (existing && ['switching','failed','reconnecting'].includes(existing.state().phase)) return true;
  existing?.dispose();
  let refs, scope, boundary, fragmentKey, initialNonce, identity, client, boundaryName;
  let phase = 'uninstalled', epoch = 0, held = [], context, banner, stopQueries, stopInputs;
  let stopAccessInputs, retainAccessInputs, accessAtom, connectedEpoch = -1, identityMustChange = false, contextPreserved = null;
  const patchedQueries = new Map();
  const originals = new Map();
  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
  const fail = () => { throw Error('ui_adapter_unavailable'); };

  function root() {
    const element = document.getElementById('root');
    if (!element) return fail();
    const name = Object.keys(element).find(name => name.startsWith('__reactContainer$'));
    const fiber = name && element[name];
    if (!fiber?.stateNode?.current) return fail();
    return {element, fiber:fiber.stateNode.current};
  }
  function fibers(start) {
    const result = [], stack = [start];
    while (stack.length) {
      const node = stack.pop(); result.push(node);
      if (result.length > 100000) return fail();
      if (node.sibling) stack.push(node.sibling);
      if (node.child) stack.push(node.child);
    }
    return result;
  }
  function locate() {
    const tree = root(), candidates = fibers(tree.fiber).filter(node => boundaryName && node.type?.name === boundaryName);
    if (candidates.length !== 1) return fail();
    return candidates[0];
  }
  function currentScope(node) {
    const candidates = fibers(node.child).filter(fiber => fiber.type === refs.gf && fiber.memoizedProps?.scope === refs.$);
    if (candidates.length !== 1) return fail();
    for (let hook = candidates[0].memoizedState; hook; hook = hook.next) {
      const value = hook.memoizedState?.current;
      if (value?.node?.token === refs.$ && value.chain instanceof Map)
        return refs.Qd(refs.$, value.chain, value.node);
    }
    return fail();
  }
  function wrapQuery(query) {
    if (patchedQueries.has(query)) return;
    const original = query.fetch;
    function fetch(...args) {
      if (phase !== 'switching' && phase !== 'failed') return original.apply(this,args);
      const generation = epoch;
      return new Promise((resolve,reject) => held.push({resolve,reject,query:this,args,generation,original}));
    }
    query.fetch = fetch; patchedQueries.set(query,{original,fetch});
  }
  function releaseQueries() {
    const requests = held; held = [];
    for (const request of requests) {
      if (request.generation !== epoch) request.reject(new DOMException('Retired account query','AbortError'));
      else Promise.resolve().then(() => request.original.apply(request.query,request.args)).then(request.resolve,request.reject);
    }
  }
  function capture() {
    const element = root().element, selection = window.getSelection();
    const routes=fibers(root().fiber).filter(node=>node.type===refs.gf && node.memoizedProps?.scope===refs.OS).map(node=>node.memoizedProps.value);
    if(routes.some(route=>['remote-thread','chatgpt-thread'].includes(route?.routeKind)))throw Error('ui_scope_unsupported');
    const manager=scope.get(refs.UE,'local');
    const threads=routes.filter(route=>route?.routeKind==='local-thread').map(route=>route.conversationId)
      .filter(id=>typeof id==='string' && manager.getConversation(id)?.resumeState==='resumed');
    return {
      route:location.href, element, active:document.activeElement,
      threads,
      range:selection?.rangeCount ? selection.getRangeAt(0).cloneRange() : null,
      fields:[...element.querySelectorAll('textarea,input:not([type=password]),[contenteditable=true]')].map(node => ({
        node, value:node.isContentEditable ? node.textContent : node.value,
        start:node.selectionStart, end:node.selectionEnd, direction:node.selectionDirection,
      })),
      scroll:[element,...element.querySelectorAll('*')].filter(node => node.scrollTop || node.scrollLeft)
        .map(node => ({node,top:node.scrollTop,left:node.scrollLeft})),
    };
  }
  function restorePosition() {
    if (!context || context.route !== location.href || root().element !== context.element) return false;
    for (const field of context.fields) {
      if (!field.node.isConnected || (field.node.isContentEditable ? field.node.textContent : field.node.value) !== field.value) return false;
    }
    for (const saved of context.scroll) if (saved.node.isConnected) {saved.node.scrollTop=saved.top;saved.node.scrollLeft=saved.left;}
    if (context.active?.isConnected) context.active.focus({preventScroll:true});
    for (const field of context.fields) if (field.node === context.active && field.start != null)
      field.node.setSelectionRange(field.start,field.end,field.direction);
    if (!['INPUT','TEXTAREA'].includes(context.active?.tagName) && context.range?.startContainer.isConnected && context.range.endContainer.isConnected) {
      const selection=window.getSelection();selection.removeAllRanges();selection.addRange(context.range);
    }
    return true;
  }
  function block() {
    banner = document.createElement('div');
    banner.setAttribute('role','status');
    banner.textContent = '正在切换账号并重新连接服务；当前页面与草稿保留，请稍候。';
    banner.style.cssText = 'position:fixed;top:12px;left:50%;transform:translateX(-50%);z-index:2147483647;padding:10px 18px;border-radius:10px;background:#24272b;color:#f3f3f3;font:13px system-ui;box-shadow:0 2px 14px #0006;pointer-events:none';
    document.body.appendChild(banner);
    const events=['keydown','beforeinput','click','pointerdown','submit','paste','drop'];
    const stop=event=>{event.preventDefault();event.stopImmediatePropagation();};
    for(const event of events)window.addEventListener(event,stop,true);
    stopInputs=()=>{for(const event of events)window.removeEventListener(event,stop,true);};
  }
  function unblock() {stopInputs?.();stopInputs=undefined;banner?.remove();banner=undefined;context=undefined;}
  function intact() {return currentScope(locate()).node === scope.node;}
  function current(generation) {
    if(generation!==epoch || !['switching','failed','reconnecting'].includes(phase) || !intact())throw Error('ui_adapter_unavailable');
  }
  function retire() {
    ++epoch;phase='uninstalled';contextPreserved=null;
    for(const [name,{original,wrapped}] of originals)if(refs.SSl[name]===wrapped)refs.SSl[name]=original;
    originals.clear();stopQueries?.();stopQueries=undefined;window.removeEventListener('pagehide',retire);
    for(const [query,{original,fetch}] of patchedQueries)if(query.fetch===fetch)query.fetch=original;
    patchedQueries.clear();stopAccessInputs?.();stopAccessInputs=undefined;retainAccessInputs?.();retainAccessInputs=undefined;
    for(const request of held)request.reject(new DOMException('Retired page scope','AbortError'));
    held=[];unblock();
  }

  const adapter = {
    revision:2,
    configure(name) {
      if(phase!=='uninstalled' || typeof name!=='string' || !/^[A-Za-z_$][\w$]*$/.test(name))return fail();
      boundaryName=name;return true;
    },
    discover() {return locate().type;},
    attach(values) {
      if (phase !== 'uninstalled') {
        if(intact())return {ok:true};
        this.dispose();
      }
      refs=values; boundaryName??=values._Sl?.name; boundary=locate(); scope=currentScope(boundary); client=scope.queryClient;
      const fragment=boundary.child?.child;
      if (boundary.type!==refs._Sl || fragment?.tag!==7 || typeof fragment.key!=='string' || fragment.child?.type!==refs.qwl) return fail();
      fragmentKey=fragment.key; identity=refs.Up.identityKey();
      const suffix=`:${identity}`;
      if(!fragmentKey.endsWith(suffix))return fail();
      initialNonce=fragmentKey.slice(0,-suffix.length);
      if(!client?.cancelQueries || !client?.resetQueries || !scope.get(refs.UE,'local') || !refs.Csn?.resolve) return fail();
      // Intercept this one existing identity Fragment; all other JSX keeps official behavior.
      for(const name of ['jsx','jsxs'])if(typeof refs.SSl[name]!=='function' || Object.getOwnPropertyDescriptor(refs.SSl,name)?.writable!==true)return fail();
      for(const name of ['jsx','jsxs']) {
        const original=refs.SSl[name];
        const wrapped=function(type,props,key) {
          if(type===Symbol.for('react.fragment') && props?.children?.type===refs.qwl && typeof key==='string' && phase!=='uninstalled') {
            const next=refs.Up.identityKey();
            if(!key.startsWith(`${initialNonce}:`) || (phase==='idle' && next!==identity)) {
              // A switch outside AuthManager keeps the official reset behavior.
              retire();
            } else key=fragmentKey;
          }
          return original(type,props,key);
        };
        originals.set(name,{original,wrapped});refs.SSl[name]=wrapped;
      }
      for(const query of client.getQueryCache().getAll())wrapQuery(query);
      stopQueries=client.getQueryCache().subscribe(event=>{
        if(event.type==='added')wrapQuery(event.query);
        if(event.type==='removed') {
          const saved=patchedQueries.get(event.query);
          if(saved && event.query.fetch===saved.fetch)event.query.fetch=saved.original;
          patchedQueries.delete(event.query);
          held=held.filter(request=>{
            if(request.query!==event.query)return true;
            request.reject(new DOMException('Removed account query','AbortError'));return false;
          });
        }
      });
      window.addEventListener('pagehide',retire,{once:true});
      phase='idle'; return {ok:true};
    },
    ready() {return phase!=='uninstalled' && intact();},
    validate() {capture();return true;},
    async begin(changesIdentity) {
      if(!this.ready() || !['idle','failed'].includes(phase))throw Error('ui_adapter_busy');
      if(phase==='failed')return; // Recovery reconnects; it must not discard the original context.
      context=capture(); contextPreserved=null;identityMustChange=changesIdentity; ++epoch; connectedEpoch=-1; phase='switching'; block();
      await client.cancelQueries();
    },
    async reconnect(owner) {
      if(!['switching','failed','reconnecting'].includes(phase) || !intact())throw Error('ui_adapter_unavailable');
      const generation=epoch;
      const identityReady=()=>refs.Up.isReady() && (!identityMustChange || refs.Up.identityKey()!==identity);
      // Do not await a potentially broken hosted RPC; the readiness deadline belongs here,
      // inside the renderer, so disconnecting the helper cannot leave an immortal transition.
      for(let attempt=0;attempt<100 && !identityReady();attempt++){current(generation);void refs.Up.refresh().catch(()=>{});await sleep(40);}
      current(generation);
      if(!identityReady())throw Error('ui_services_unavailable');
      if(connectedEpoch!==epoch) {
        const manager=refs.V4n(scope,'local',scope.get(refs.HE,'local'),refs.i6n.bind(null,scope),(get,id)=>get(refs.R8n,id),{getManagers:()=>scope.get(refs.WE)});
        new refs.uD(scope).addManager(manager); // Official cleanup, registry generation and RPC replacement.
        connectedEpoch=epoch;
      }
      // A broken official external-signal effect does not restart itself.
      if(scope.get(refs.Csn)==='error') {
        stopAccessInputs?.();
        accessAtom=refs.Csn.resolve(scope.node,scope.chain);
        // Retain one managed subscription so UI unmount/remount cannot create a competing effect.
        retainAccessInputs??=scope.node.store.sub(accessAtom,()=>{});
        if(scope.get(refs.Csn)==='error')stopAccessInputs=accessAtom.onMount(value=>scope.node.store.set(accessAtom,value));
      }
      for(let attempt=0;attempt<100 && (scope.get(refs.Csn)!=='ready' || !scope.get(refs.O_,'local'));attempt++){current(generation);await sleep(40);}
      current(generation);
      if(scope.get(refs.Csn)!=='ready' || !scope.get(refs.O_,'local'))throw Error('ui_services_unavailable');
      phase='reconnecting';
      // The old cache remains unusable while blocked. Official queries repopulate it only after services reconnect.
      void client.resetQueries().catch(()=>{}); current(generation);releaseQueries();
      // Official query options follow the account-input state on a later render, so the fetch started by the
      // reset can still run options computed while that state was `error`. Ask again until the new account lands.
      for(let attempt=0;attempt<375;attempt++) {
        current(generation);
        const info=client.getQueryData(['vscode','account-info']);
        const manager=scope.get(refs.UE,'local');
        const threadsReady=context.threads.every(id=>manager.getConversation(id)?.resumeState==='resumed');
        const matches=info && `codex:${info.userId}:${info.accountId}`===owner;
        if(!matches && attempt%10===9 && client.getQueryCache().find({queryKey:['vscode','account-info']})?.state.fetchStatus!=='fetching')
          void client.refetchQueries({queryKey:['vscode','account-info']}).catch(()=>{});
        if(threadsReady && matches) {
          current(generation);
          contextPreserved=restorePosition();
          identity=refs.Up.identityKey(); phase='idle'; unblock(); return {ok:true,contextPreserved};
        }
        await sleep(40);
      }
      throw Error('ui_services_unavailable');
    },
    failed() {
      if(phase==='uninstalled' || phase==='idle')return;
      phase='failed';
      if(banner)banner.textContent='账号切换尚未确认；页面与草稿已保留，请在 AuthManager 恢复当前登录状态。';
    },
    state() {return {phase,epoch,contextPreserved};},
    dispose() {
      if(['switching','failed','reconnecting'].includes(phase))throw Error('ui_adapter_busy');
      retire();
    },
  };
  globalThis[key]=adapter;
  return true;
})()
