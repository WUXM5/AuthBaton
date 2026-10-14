// Production task coordinator, synthetic protocol only. Never discovers/contacts a desktop.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import {webcrypto} from 'node:crypto';

const source=fs.readFileSync(new URL('../macos/Sources/DesktopBridge/automatic.js',import.meta.url),'utf8');
function fixture() {
  let owner='A',fail='',batch='batch-1';
  const calls:string[]=[];
  const threads=new Map(['active-1','active-2','idle','manual-paused','completed'].map((id,i)=>[id,{
    status:i<2?'active':'idle',flags:[] as string[],turn:{id:`turn-${id}`,status:i<2?'inProgress':i===3?'interrupted':'completed'},
  }]));
  let serial=0;
  const rpc=async(method:string,p:any):Promise<any>=>{
    calls.push(`${method}:${p.threadId??''}`);
    if(method==='thread/loaded/list')return {data:[...threads.keys()],nextCursor:null};
    const t=threads.get(p.threadId)!;
    if(method==='thread/read')return {thread:{status:{type:t.status,activeFlags:t.flags}}};
    if(method==='thread/turns/list')return {data:[{...t.turn}]};
    if(method==='turn/interrupt') {
      assert.equal(p.turnId,t.turn.id);
      t.status='idle';t.turn.status='interrupted';
      if(fail==='interrupt')throw Error('rpc_timeout');
      return {};
    }
    if(method==='turn/start') {
      assert.equal(owner,'B');assert.equal(t.status,'idle');
      assert.equal(p.input.length,1);assert(p.clientUserMessageId);
      assert(!('approvalPolicy' in p));assert(!('sandboxPolicy' in p));assert(!('model' in p));
      t.status='active';t.turn={id:`new-${++serial}`,status:'inProgress'};
      if(fail==='resume')throw Error('rpc_timeout');
      return {turn:{...t.turn}};
    }
    throw Error('unexpected RPC');
  };
  const context=vm.createContext({crypto:webcrypto,setTimeout,Date,globalThis:undefined});
  context.globalThis=context;
  vm.runInContext(source,context);
  const run=context.__authManagerTasksV1({rpc,instance:'renderer-1',status:async()=>({owner,mode:'chatgptAuthTokens'}),idle:async()=>{
    if([...threads.values()].some(t=>t.status!=='idle'))throw Error('tasks_active');
  }});
  return {threads,calls,owner:(v:string)=>{owner=v},fail:(v:string)=>{fail=v},
    call:(op:string,params:any={})=>run(`auto_${op}`,{batchId:batch,instance:'renderer-1',owner:'A',targetOwner:'B',...params}),
  };
}
let passed=0;
async function test(name:string,fn:()=>Promise<void>):Promise<void>{await fn();passed++;console.log(`PASS ${name}`);}
await test('two active sessions resume once; idle, manual-paused and completed remain untouched',async()=>{
  const f=fixture();const plan=await f.call('prepare');assert.equal(plan.tasks.length,2);
  for(const t of plan.tasks)await f.call('pause',{threadId:t.thread_id});
  await f.call('verify');f.owner('B');
  for(const t of plan.tasks){await f.call('resume',{threadId:t.thread_id});await f.call('resume',{threadId:t.thread_id});}
  assert.equal(f.calls.filter(c=>c.startsWith('turn/start')).length,2);
  assert.equal(f.calls.filter(c=>c.startsWith('turn/interrupt')).length,2);
  for(const id of ['idle','manual-paused','completed'])assert(!f.calls.some(c=>c===`turn/start:${id}`));
});
await test('a systemError thread counts only while its latest turn is still running',async()=>{
  const f=fixture();
  f.threads.set('errored',{status:'systemError',flags:[],turn:{id:'turn-errored',status:'failed'}});
  const plan=await f.call('prepare');
  assert.deepEqual([...plan.tasks.map((t:any)=>t.thread_id)].sort(),['active-1','active-2']);
  assert(!f.calls.some(c=>c==='turn/interrupt:errored'));
  const g=fixture();
  g.threads.set('errored',{status:'systemError',flags:[],turn:{id:'turn-errored',status:'inProgress'}});
  await assert.rejects(g.call('prepare'),/tasks_waiting/);
  assert(!g.calls.some(c=>c.startsWith('turn/interrupt')));
});
await test('pending approval stops before interruption',async()=>{
  const f=fixture();f.threads.get('active-2')!.flags=['waitingOnApproval'];
  await assert.rejects(f.call('prepare'),/tasks_waiting/);
  assert(!f.calls.some(c=>c.startsWith('turn/interrupt')));
});
await test('natural completion or manual stop before our interrupt is never resumed',async()=>{
  const f=fixture();await f.call('prepare');
  f.threads.get('active-1')!.status='idle';f.threads.get('active-1')!.turn.status='completed';
  f.threads.get('active-2')!.status='idle';f.threads.get('active-2')!.turn.status='interrupted';
  await f.call('pause',{threadId:'active-1'});await f.call('pause',{threadId:'active-2'});
  await f.call('verify');f.owner('B');await f.call('resume',{threadId:'active-2'});
  assert(!f.calls.some(c=>c.startsWith('turn/start')||c.startsWith('turn/interrupt')));
});
await test('new turn before pause cancels batch; new activity before commit blocks switching',async()=>{
  const f=fixture();await f.call('prepare');f.threads.get('active-1')!.turn.id='other';
  await assert.rejects(f.call('pause',{threadId:'active-1'}),/tasks_changed/);
  assert(!f.calls.some(c=>c.startsWith('turn/interrupt')));
  const g=fixture();await g.call('prepare');await g.call('pause',{threadId:'active-1'});await g.call('pause',{threadId:'active-2'});
  g.threads.get('idle')!.status='active';await assert.rejects(g.call('verify'),/tasks_active/);
});
await test('lost interrupt ack is never replayed or assumed paused',async()=>{
  const f=fixture();await f.call('prepare');f.fail('interrupt');
  await assert.rejects(f.call('pause',{threadId:'active-1'}),/rpc_timeout/);
  const result=await f.call('pause',{threadId:'active-1'});
  assert.equal(result.tasks[0].state,'interrupt_sent');
  await assert.rejects(f.call('verify'),/pause_unconfirmed/);
  assert.equal(f.calls.filter(c=>c.startsWith('turn/interrupt')).length,1);
});
await test('lost resume ack cannot start a duplicate turn',async()=>{
  const f=fixture();await f.call('prepare');await f.call('pause',{threadId:'active-1'});f.owner('B');f.fail('resume');
  await assert.rejects(f.call('resume',{threadId:'active-1'}),/rpc_timeout/);
  const result=await f.call('resume',{threadId:'active-1'});assert.equal(result.tasks[0].state,'resume_sent');
  assert.equal(f.calls.filter(c=>c.startsWith('turn/start')).length,1);
});
await test('wrong identity or replaced renderer cannot resume',async()=>{
  const f=fixture();await f.call('prepare');await f.call('pause',{threadId:'active-1'});
  await assert.rejects(f.call('resume',{threadId:'active-1'}),/identity_changed/);
  await assert.rejects(f.call('resume',{threadId:'active-1',instance:'restarted'}),/batch_unavailable/);
  assert(!f.calls.some(c=>c.startsWith('turn/start')));
});
await test('user continued a paused session before us; skip instead of steering',async()=>{
  const f=fixture();await f.call('prepare');await f.call('pause',{threadId:'active-1'});f.owner('B');
  f.threads.get('active-1')!.turn={id:'manual',status:'inProgress'};f.threads.get('active-1')!.status='active';
  const value=await f.call('resume',{threadId:'active-1'});assert.equal(value.tasks[0].state,'skipped');
  assert(!f.calls.some(c=>c.startsWith('turn/start')));
});
await test('cancel releases receipt without resuming or changing authentication',async()=>{
  const f=fixture();await f.call('prepare');await f.call('pause',{threadId:'active-1'});
  await f.call('cancel');await assert.rejects(f.call('resume',{threadId:'active-1'}),/batch_unavailable/);
  assert(!f.calls.some(c=>c.startsWith('turn/start')));
});
console.log(`${passed} automatic desktop cases passed; real desktop operations: 0`);
