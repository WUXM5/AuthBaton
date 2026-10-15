// Desktop-only task coordination. No auth material, storage, process restart or UI navigation.
(() => {
  if (globalThis.__authManagerTasksV1?.revision===5) return true;
  globalThis.__authManagerTasksV1 = ({rpc, status, idle, instance}) => {
    let batch = null, generation = 0, preparing = null;
    const valid = id => typeof id === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(id);
    // The desktop's own background generations are never paused or resumed: resuming a next-message
    // prediction would start a hidden turn on a copy of the conversation. Same list as preflight.js.
    const background = thread => thread?.ephemeral===true && (thread.threadSource==null || [
      'ambient_suggestions','ambient_suggestion_safety','safety_recourse','mcp_extension_host','composer_predictions',
      'thread_title','thread_description','thread_summary','commit_message','commit_pull_request_message',
      'pull_request_message','dictation_cleanup','inline_edit','scheduled_setup_advice','ads_manager_performance_summary',
    ].includes(thread.threadSource));
    const read = async id => {
      const {thread} = await rpc('thread/read', {threadId:id,includeTurns:false});
      const page = await rpc('thread/turns/list', {threadId:id,limit:1,sortDirection:'desc',itemsView:'summary'});
      if (!thread?.status || !Array.isArray(page.data)) throw Error('task_probe_failed');
      return {thread,turn:page.data[0]};
    };
    async function loaded() {
      let cursor = null; const ids = [];
      do {
        const page = await rpc('thread/loaded/list',{cursor,limit:100});
        if (!Array.isArray(page.data) || page.data.some(id=>!valid(id))) throw Error('task_probe_failed');
        ids.push(...page.data);
        if(ids.length>256 || new Set(ids).size!==ids.length)throw Error('task_probe_failed');
        cursor=page.nextCursor;
      } while(cursor);
      return ids;
    }
    function check(p) {
      if(!batch || batch.id!==p.batchId || p.instance!==instance)throw Error('batch_unavailable');
      return batch;
    }
    const alive = b => { if(batch!==b || b.generation!==generation)throw Error('batch_unavailable'); };
    const output = current => ({...current,instance,tasks:batch.tasks.map(t=>({...t}))});
    async function identity(owner) {
      const current=await status();
      if(current.owner!==owner)throw Error('identity_changed');
      return current;
    }
    return async (operation,p) => {
      if(operation==='auto_prepare') {
        if(!valid(p.batchId) || typeof p.owner!=='string' || typeof p.targetOwner!=='string')throw Error('invalid_batch');
        // Only a batch explicitly retired by the user's recovery acknowledgement is discarded.
        if(valid(p.retireBatch) && (batch?.id===p.retireBatch || preparing===p.retireBatch)) {
          generation++;batch=null;preparing=null;
        }
        if(preparing || (batch && batch.id!==p.batchId))throw Error('batch_busy');
        const epoch=generation;preparing=p.batchId;
        try {
        const current=await identity(p.owner);
        if(epoch!==generation)throw Error('batch_unavailable');
        if(batch)return output(current);
        const tasks=[];
        for(const id of await loaded()) {
          const {thread,turn}=await read(id);
          // systemError lasts until the next turn starts; only a turn still in progress is a task to carry over.
          if(background(thread) || ['idle','notLoaded'].includes(thread.status.type) || (thread.status.type==='systemError' && turn?.status!=='inProgress'))continue;
          if(thread.status.type!=='active' || thread.status.activeFlags?.length)throw Error('tasks_waiting');
          if(!valid(turn?.id) || turn.status!=='inProgress')throw Error('task_probe_failed');
          tasks.push({thread_id:id,turn_id:turn.id,resume_key:crypto.randomUUID(),state:'planned',resumed_turn_id:null});
        }
        await identity(p.owner);
        if(epoch!==generation)throw Error('batch_unavailable');
        batch={id:p.batchId,owner:p.owner,targetOwner:p.targetOwner,tasks,generation:epoch,working:false};
        return output(current);
        } finally {if(preparing===p.batchId)preparing=null;}
      }
      if(operation==='auto_cancel') {
        // Cancellation never retries a task or performs an auth write.
        if(batch && batch.id!==p.batchId)throw Error('batch_unavailable');
        generation++;batch=null;preparing=null;return {instance,cancelled:true};
      }
      const b=check(p);
      if(operation==='auto_status'){const current=await status();alive(b);return output(current);}
      if(b.working)throw Error('batch_busy');
      b.working=true;
      try {
      if(operation==='auto_pause') {
        await identity(b.owner);alive(b);
        const task=b.tasks.find(t=>t.thread_id===p.threadId);
        if(!task)throw Error('invalid_batch');
        if(task.state!=='planned')return output(await status());
        const {thread,turn}=await read(task.thread_id);alive(b);
        if(turn?.id!==task.turn_id)throw Error('tasks_changed');
        if(thread.status.type==='idle' && turn.status!=='inProgress') {
          task.state='skipped';return output(await status());
        }
        if(thread.status.type!=='active' || thread.status.activeFlags?.length || turn.status!=='inProgress')throw Error('tasks_changed');
        task.state='interrupt_sent'; // At most one interrupt, even if the RPC response is lost.
        await rpc('turn/interrupt',{threadId:task.thread_id,turnId:task.turn_id});alive(b);
        const deadline=Date.now()+10000;
        for(let i=0;i<30;i++) {
          const next=await read(task.thread_id);alive(b);
          if(next.turn?.id!==task.turn_id)throw Error('tasks_changed');
          if(next.thread.status.type==='idle' && next.turn.status!=='inProgress') {
            task.state=next.turn.status==='interrupted'?'paused':'skipped';
            const current=await identity(b.owner);alive(b);return output(current);
          }
          if(Date.now()>=deadline)break;
          await new Promise(r=>setTimeout(r,100));alive(b);
        }
        throw Error('pause_unconfirmed');
      }
      if(operation==='auto_verify') {
        const current=await identity(b.owner);alive(b);
        if(b.tasks.some(t=>!['paused','skipped'].includes(t.state)))throw Error('pause_unconfirmed');
        for(const task of b.tasks) {
          const next=await read(task.thread_id);alive(b);
          if(next.turn?.id!==task.turn_id || next.thread.status.type!=='idle')throw Error('tasks_changed');
        }
        await idle();alive(b);return output(current);
      }
      if(operation==='auto_resume') {
        const current=await identity(b.targetOwner);alive(b);
        const task=b.tasks.find(t=>t.thread_id===p.threadId);
        if(!task)throw Error('invalid_batch');
        if(task.state!=='paused')return output(current);
        const next=await read(task.thread_id);alive(b);
        if(next.turn?.id!==task.turn_id || next.thread.status.type!=='idle' || next.turn.status!=='interrupted') {
          task.state='skipped';return output(current); // User/new activity wins; never steer it.
        }
        task.state='resume_sent'; // No second turn/start after an unknown response.
        const result=await rpc('turn/start',{
          threadId:task.thread_id,clientUserMessageId:task.resume_key,
          input:[{type:'text',text:'AuthManager 已按你启用的低额度策略暂停本轮并切换账号。请保留此会话的原目标、约束和权限，先核对中断前工具操作的实际结果，避免重复已完成的操作，然后继续尚未完成的任务。'}],
        });
        alive(b);
        if(!valid(result.turn?.id) || result.turn.id===task.turn_id)throw Error('resume_unconfirmed');
        task.resumed_turn_id=result.turn.id;task.state='resumed';
        const final=await identity(b.targetOwner);alive(b);return output(final);
      }
      throw Error('unknown_operation');
      } finally {b.working=false;}
    };
  };
  globalThis.__authManagerTasksV1.revision = 5;
  return true;
})();
