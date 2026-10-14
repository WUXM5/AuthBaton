// Production automatic.js loaded into a synthetic VM. No desktop, credentials or network.
import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
import {webcrypto} from 'node:crypto';
const source=fs.readFileSync(new URL('../macos/Sources/DesktopBridge/automatic.js',import.meta.url),'utf8');
function fixture() {
  let owner='A';const calls:string[]=[];
  const thread={status:'active',turn:{id:'original-turn',status:'inProgress'}};
  let delayed=false,signal:()=>void=()=>{},unblock:()=>void=()=>{};
  const entered=new Promise<void>(r=>signal=r),release=new Promise<void>(r=>unblock=r);
  let serial=0;
  const rpc=async(method:string,p:any):Promise<any>=>{
    calls.push(method);
    if(method==='thread/loaded/list')return {data:['one'],nextCursor:null};
    if(method==='thread/read') {
      const snapshot={thread:{status:{type:thread.status,activeFlags:[]}}};
      if(delayed){signal();await release;}
      return snapshot;
    }
    if(method==='thread/turns/list')return {data:[{...thread.turn}]};
    if(method==='turn/interrupt'){thread.status='idle';thread.turn.status='interrupted';return {};}
    if(method==='turn/start'){thread.status='active';thread.turn={id:`new-${++serial}`,status:'inProgress'};return {turn:{...thread.turn}};}
    throw Error('unexpected');
  };
  const ctx=vm.createContext({crypto:webcrypto,setTimeout,Date});
  vm.runInContext(source,ctx);
  const run=ctx.__authManagerTasksV1({rpc,instance:'renderer',status:async()=>({owner}),idle:async()=>{}});
  return {calls,owner:(s:string)=>{owner=s},delay:()=>{delayed=true},entered,release:unblock,
    call:(op:string,p:any={})=>run(`auto_${op}`,{batchId:'batch',instance:'renderer',owner:'A',targetOwner:'B',threadId:'one',...p})};
}
const results:any[]=[];
{
  const f=fixture();await f.call('prepare');
  let failure='';try{await f.call('prepare',{batchId:'new-batch'});}catch(e){failure=(e as Error).message;}
  assert.equal(failure,'batch_busy');
  results.push({name:'lost prepare acknowledgement leaves existing renderer batch rejecting newly generated id',passed:true,failure});
}
{
  const f=fixture();await f.call('prepare');await f.call('pause');f.owner('B');f.delay();
  const pending=f.call('resume');await f.entered;await f.call('cancel');f.release();
  await pending.catch(()=>{});
  const starts=f.calls.filter(c=>c==='turn/start').length;
  assert.equal(starts,0);
  results.push({name:'cancel while resume read is pending does not send turn/start after cancel',passed:true,startsAfterCancellation:starts});
}
{
  const f=fixture();await f.call('prepare');f.delay();
  const pending=f.call('pause');await f.entered;await f.call('cancel');f.release();
  await pending.catch(()=>{});
  const interrupts=f.calls.filter(c=>c==='turn/interrupt').length;
  assert.equal(interrupts,0);
  results.push({name:'cancel while pause read is pending does not send turn/interrupt after cancel',passed:true,interruptsAfterCancellation:interrupts});
}
{
  const f=fixture();await f.call('prepare');await f.call('pause');f.owner('B');f.delay();
  const first=f.call('resume');await f.entered;const second=f.call('resume').catch(e=>e);
  await new Promise<void>(r=>setTimeout(r,0));f.release();
  await Promise.allSettled([first,second]);
  const starts=f.calls.filter(c=>c==='turn/start').length;
  assert.equal(starts,1);
  results.push({name:'two overlapping resume calls pass pre-await state check and permit only one turn/start',passed:true,starts});
}
{
  const f=fixture();f.delay();
  const pending=f.call('prepare');await f.entered;await f.call('cancel');f.release();
  await assert.rejects(pending,/batch_unavailable/);
  await assert.rejects(f.call('status'),/batch_unavailable/);
  results.push({name:'cancel invalidates an in-flight prepare before its batch is installed',passed:true});
}
{
  const f=fixture();await f.call('prepare');
  await f.call('prepare',{batchId:'replacement',retireBatch:'batch'});
  await assert.rejects(f.call('status'),/batch_unavailable/);
  assert.equal((await f.call('status',{batchId:'replacement'})).tasks.length,1);
  assert.equal(f.calls.includes('turn/start'),false);assert.equal(f.calls.includes('turn/interrupt'),false);
  results.push({name:'explicit recovery acknowledgement retires only the old batch without task actions',passed:true});
}
console.log(JSON.stringify({realDesktopCalls:0,results},null,2));
