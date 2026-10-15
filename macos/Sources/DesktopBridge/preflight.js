// A short-lived reader, compatible with the already-installed V1 bridge.
// It never replaces that bridge or changes its authentication binding.
(async () => {
  const pending = new Map();
  const instance = crypto.randomUUID();
  let serial = 0;
  const receive = event => {
    const message = event.data;
    if (message?.type !== 'mcp-response' || message.hostId !== 'local') return;
    const response = message.message;
    const request = pending.get(response?.id);
    if (!request) return;
    clearTimeout(request.timer); pending.delete(response.id);
    response.error ? request.reject(Error('task_probe_failed')) : request.resolve(response.result);
  };
  const rpc = (method, params) => new Promise((resolve, reject) => {
    const id = `authmanager-preflight:${instance}:${++serial}`;
    const timer = setTimeout(() => {pending.delete(id);reject(Error('task_probe_failed'));}, 4500);
    pending.set(id, {resolve, reject, timer});
    window.electronBridge.sendMessageFromView({type:'mcp-request',hostId:'local',request:{id,method,params}})
      .catch(() => {clearTimeout(timer);pending.delete(id);reject(Error('task_probe_failed'));});
  });
  // Background generations the desktop runs for itself use ephemeral threads whose source is unset or one of
  // these (desktop 26.1007): ambient suggestions and their safety check, safety recourse, the MCP extension
  // host, the next-message prediction after each turn, thread titles and summaries, commit and pull request
  // messages, dictation cleanup, inline edits, schedule advice and the ads summary. The user never sees them
  // and nothing in them needs finishing. A side chat is ephemeral too but carries `user`, and an unknown
  // source still counts. Same list in bridge.js and automatic.js.
  const background = thread => thread?.ephemeral === true && (thread.threadSource == null || [
    'ambient_suggestions','ambient_suggestion_safety','safety_recourse','mcp_extension_host','composer_predictions',
    'thread_title','thread_description','thread_summary','commit_message','commit_pull_request_message',
    'pull_request_message','dictation_cleanup','inline_edit','scheduled_setup_advice','ads_manager_performance_summary',
  ].includes(thread.threadSource));
  // Same rule as bridge.js: a systemError thread is busy only while its latest turn is still in progress.
  const running = async (threadId, thread) => {
    const status = thread?.status;
    if (background(thread) || ['idle','notLoaded'].includes(status?.type)) return false;
    if (status?.type !== 'systemError') return true;
    const page = await rpc('thread/turns/list', {threadId,limit:1,sortDirection:'desc',itemsView:'notLoaded'});
    if (!Array.isArray(page.data)) throw Error('task_probe_failed');
    return page.data[0]?.status === 'inProgress';
  };
  window.addEventListener('message', receive);
  try {
    const current = await globalThis.__authManagerDesktopV1.call({operation:'probe',params:{}});
    if (!current?.ok) return current;
    // Name the busy threads so the user knows which task to finish; the rule matches the bridge's apply check.
    const busy = [];
    let cursor = null;
    for (let page = 0; page < 100; page++) {
      const list = await rpc('thread/loaded/list', {cursor,limit:100});
      if (!Array.isArray(list.data)) throw Error('task_probe_failed');
      for (const threadId of list.data) {
        const {thread} = await rpc('thread/read', {threadId,includeTurns:false});
        if (await running(threadId, thread)) {
          busy.push({id:String(threadId), name:String(thread?.name || thread?.preview || '').slice(0,40), status:String(thread?.status?.type), flags:thread?.status?.activeFlags ?? [], source:thread?.threadSource ?? null});
        }
      }
      if (!list.nextCursor) return busy.length ? {ok:false,code:'tasks_active',tasks:busy.slice(0,3)} : current;
      cursor = list.nextCursor;
    }
    throw Error('task_probe_failed');
  } catch (error) {
    return {ok:false,code:error.message === 'tasks_active' ? 'tasks_active' : 'task_probe_failed'};
  } finally {
    window.removeEventListener('message', receive);
    for (const request of pending.values()) clearTimeout(request.timer);
  }
})()
