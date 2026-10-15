// Runs only in the inspected Codex primary renderer. Never logs credentials.
(async () => {
  const key = '__authManagerDesktopV1';
  if (globalThis[key]?.revision===10) return true;
  if (globalThis[key]) {
    const old=await globalThis[key].call({operation:'poll',params:{}});
    if(!old.ok || old.requests?.length)throw Error('bridge_upgrade_busy');
    await globalThis[key].call({operation:'release',params:{}});
  }
  // Releases before 0.18 installed a page adapter that kept the page through a switch. An idle one is
  // removed; one still holding the page is handed back to Codex with a reload, as it did itself on failure.
  const adapter=globalThis.__authManagerSelectiveUIV1;
  if (adapter) {
    if (['idle','uninstalled'].includes(adapter.state?.().phase)) adapter.dispose();
    else setTimeout(() => location.reload(), 200);
    delete globalThis.__authManagerSelectiveUIV1;
  }
  const send = (value) => window.electronBridge.sendMessageFromView(value);
  const pending = new Map();
  const refreshes = new Map();
  let serial = 0, epoch = 0, binding = null, completed = null, updated = false;
  const instance = crypto.randomUUID();
  let observedOwner=null, usageObservation=null;
  const rpc = (method, params) => new Promise((resolve, reject) => {
    const id = `authmanager:${instance}:${++serial}`;
    const timer = setTimeout(() => { pending.delete(id); reject(Error('rpc_timeout')); }, 4500);
    pending.set(id, {resolve, reject, timer});
    send({type:'mcp-request', hostId:'local', request:{id, method, params}})
      .catch(() => {clearTimeout(timer); pending.delete(id); reject(Error('rpc_failed'));});
  });
  function identity(token) {
    const payload = token.split('.')[1];
    const claims = JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(payload.replace(/-/g,'+').replace(/_/g,'/')), c => c.charCodeAt(0))));
    const a = claims['https://api.openai.com/auth'];
    const user = a?.chatgpt_user_id ?? a?.user_id;
    const workspace = a?.chatgpt_account_id;
    if (!user || !workspace) throw Error('identity_missing');
    return {owner:`codex:${user}:${workspace}`, workspace, expiresAt:claims.exp};
  }
  async function status() {
    const value = await rpc('getAuthStatus', {includeToken:true, refreshToken:false});
    if (!['chatgpt','chatgptAuthTokens'].includes(value.authMethod) || !value.authToken) throw Error('unsupported_auth');
    const hash = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',new TextEncoder().encode(value.authToken))), b=>b.toString(16).padStart(2,'0')).join('');
    const owner=identity(value.authToken).owner;
    if(observedOwner!==owner)usageObservation=null;
    observedOwner=owner;
    return {...identity(value.authToken), mode:value.authMethod, hash};
  }
  // The desktop's own background generations (ephemeral, source unset or one of these) are never tasks;
  // a side chat is ephemeral too but carries `user`. Same list as preflight.js, which names each source.
  const background = thread => thread?.ephemeral===true && (thread.threadSource==null || [
    'ambient_suggestions','ambient_suggestion_safety','safety_recourse','mcp_extension_host','composer_predictions',
    'thread_title','thread_description','thread_summary','commit_message','commit_pull_request_message',
    'pull_request_message','dictation_cleanup','inline_edit','scheduled_setup_advice','ads_manager_performance_summary',
  ].includes(thread.threadSource));
  // systemError only records that the latest turn hit an error and lasts until the next turn starts,
  // so for it the turn itself says whether anything is still running. Matches preflight.js.
  async function running(threadId, thread) {
    const status=thread?.status;
    if (background(thread) || ['idle','notLoaded'].includes(status?.type)) return false;
    if (status?.type!=='systemError') return true;
    const page = await rpc('thread/turns/list', {threadId, limit:1, sortDirection:'desc', itemsView:'notLoaded'});
    if (!Array.isArray(page.data)) throw Error('task_probe_failed');
    return page.data[0]?.status==='inProgress';
  }
  async function idle() {
    let cursor = null;
    for (let page=0; page<100; page++) {
      const list = await rpc('thread/loaded/list', {cursor, limit:100});
      if (!Array.isArray(list.data)) throw Error('invalid_threads');
      for (const threadId of list.data) {
        const {thread} = await rpc('thread/read', {threadId, includeTurns:false});
        if (await running(threadId, thread)) throw Error('tasks_active');
      }
      if (!list.nextCursor) return;
      cursor = list.nextCursor;
    }
    throw Error('too_many_threads');
  }
  async function errorReply(id) {
    await send({type:'mcp-response', hostId:'local', response:{id,error:{code:-32000,message:'BatonPass refresh unavailable; retry after maintenance'}}});
  }
  const automatic=globalThis.__authManagerTasksV1?.({rpc,status,idle,instance});
  window.addEventListener('message', (event) => {
    const m=event.data;
    if (!m || m.hostId!=='local') return;
    if (m.type==='mcp-response') {
      const p=pending.get(m.message?.id);
      if (p) {clearTimeout(p.timer); pending.delete(m.message.id); m.message.error ? p.reject(Error('rpc_failed')) : p.resolve(m.message.result);}
    }
    if (m.type==='mcp-notification') {
      if (m.method==='account/login/completed' && m.params?.loginId==null) completed=m.params.success===true;
      if (m.method==='account/updated') {updated=true;usageObservation=null;observedOwner=null;}
      if(m.method==='account/rateLimits/updated' && observedOwner) {
        const p=m.params;
        const bucket=p?.rateLimitsByLimitId?.codex ?? p?.rateLimits;
        if(bucket && (!bucket.limitId || bucket.limitId==='codex')) {
          const windows=[bucket.primary,bucket.secondary].filter(Boolean);
          if(windows.length && windows.every(w=>Number.isFinite(w.usedPercent) && w.usedPercent>=0 && w.usedPercent<=100 && Number.isFinite(w.resetsAt) && w.resetsAt*1000>Date.now()))
            usageObservation={owner:observedOwner,checked_at:Math.floor(Date.now()/1000),remaining:Math.min(...windows.map(w=>100-w.usedPercent)),expires_at:Math.min(...windows.map(w=>w.resetsAt))};
        }
      }
    }
    if (m.type==='mcp-request' && m.request?.method==='account/chatgptAuthTokens/refresh') {
      const r=m.request;
      if (!binding) return;
      if (r.params?.reason!=='unauthorized' || r.params.previousAccountId!==binding.workspace) {void errorReply(r.id).catch(()=>{});return;}
      refreshes.set(String(r.id), {id:r.id, epoch, created:Date.now(), workspace:binding.workspace});
    }
  });
  // Codex rebuilds its own page on account/updated. When its online account check fails it reports
  // success:false and sends no account/updated, yet keeps the new tokens: the page would go on showing
  // the old account, so it reloads onto the one the app-server holds. After the bridge has answered.
  async function settle(previousOwner) {
    if (updated) return;
    let changed=true;
    try {changed=(await status()).owner!==previousOwner;} catch {}
    if (changed) setTimeout(() => location.reload(), 200);
  }
  globalThis[key] = {
    revision:10,
    async call(input) {
      let previousOwner=null, loginSent=false;
      try {
        if(input.operation.startsWith('auto_')) {
          if(!automatic)throw Error('bridge_failed');
          return {ok:true,...await automatic(input.operation,input.params)};
        }
        if (input.operation==='probe') return {ok:true, instance, boundOwner:binding?.owner??null, ...(await status())};
        if (input.operation==='bind') {
          const current=await status();
          if (current.owner!==input.params.owner || current.mode!=='chatgptAuthTokens') throw Error('identity_changed');
          ++epoch;binding={owner:current.owner,workspace:current.workspace};
          return {ok:true,instance,...current,boundOwner:binding.owner,confirmed:true};
        }
        if (input.operation==='apply' || input.operation==='update') {
          const before=await status(), p=input.params;
          previousOwner=before.owner;
          if (before.owner!==p.previousOwner && before.owner!==p.owner) throw Error('identity_changed');
          if (refreshes.size) throw Error('refresh_pending');
          if (input.operation==='apply') await idle();
          else if (before.owner!==p.owner || before.mode!=='chatgptAuthTokens') throw Error('identity_changed');
          const tokenIdentity=identity(p.auth.accessToken);
          if (tokenIdentity.owner!==p.owner || tokenIdentity.workspace!==p.auth.chatgptAccountId) throw Error('identity_mismatch');
          ++epoch; binding={...tokenIdentity}; completed=null; updated=false;
          loginSent=true;
          const result=await rpc('account/login/start',{type:'chatgptAuthTokens',...p.auth});
          if (result?.type!=='chatgptAuthTokens') throw Error('invalid_login');
          // RPC ack precedes routing validation. Failure is a partially applied operation.
          // Completion waits for Codex to read the account online: 0.9–1.95 s in ten real switches.
          for (let i=0; i<160 && completed===null; i++) await new Promise(r=>setTimeout(r,50));
          const current=await status();
          const account=await rpc('account/read',{refreshToken:false});
          const confirmed=current.owner===p.owner && current.mode==='chatgptAuthTokens' && completed===true && updated && account.account?.type==='chatgpt';
          if (!confirmed) await settle(before.owner);
          return {ok:true, instance, ...current, boundOwner:binding.owner, confirmed};
        }
        if (input.operation==='poll') {
          const current=await status();
          const requests=[];
          for (const [key,r] of refreshes) {
            if (Date.now()-r.created>7500) {refreshes.delete(key);void errorReply(r.id).catch(()=>{});}
            else requests.push({...r, ageMs:Date.now()-r.created});
          }
          return {ok:true,instance,...current,boundOwner:binding?.owner??null,requests,usageObservation};
        }
        if (input.operation==='reply') {
          const p=input.params, r=refreshes.get(String(p.id));
          if (p.instance!==instance || !r || r.epoch!==p.epoch || r.epoch!==epoch || Date.now()-r.created>8000) throw Error('stale_refresh');
          if (!p.auth) {await errorReply(r.id);refreshes.delete(String(r.id));return {ok:true};}
          const current=await status(), next=identity(p.auth.accessToken);
          if (!binding || current.owner!==binding.owner || next.owner!==binding.owner || p.auth.chatgptAccountId!==binding.workspace) throw Error('identity_changed');
          await send({type:'mcp-response',hostId:'local',response:{id:r.id,result:p.auth}});
          // Keep the binding busy until the backend has committed the returned generation.
          const expected=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',new TextEncoder().encode(p.auth.accessToken))),b=>b.toString(16).padStart(2,'0')).join('');
          for (let i=0;i<30;i++) {
            const actual=await status();
            if (actual.owner===binding.owner && actual.hash===expected) {refreshes.delete(String(r.id));return {ok:true};}
            await new Promise(resolve=>setTimeout(resolve,50));
          }
          throw Error('refresh_unconfirmed');
        }
        if (input.operation==='release') {
          ++epoch;binding=null;
          for(const r of refreshes.values()) await errorReply(r.id);
          refreshes.clear();return {ok:true};
        }
        throw Error('unknown_operation');
      } catch(e) {
        if (loginSent) await settle(previousOwner);
        const known=['tasks_active','tasks_waiting','tasks_changed','pause_unconfirmed','resume_unconfirmed','batch_unavailable','batch_busy','invalid_batch','task_probe_failed','identity_changed','identity_mismatch','refresh_pending','stale_refresh','refresh_unconfirmed','unsupported_auth'];
        return {ok:false,code:[...known,'rpc_timeout','rpc_failed'].includes(e.message)?e.message:'bridge_failed'};
      }
    }
  };
  return true;
})()
