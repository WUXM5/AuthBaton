// Synthetic renderer + protocol peer; uses the exact JavaScript shipped in the helper.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { webcrypto } from 'node:crypto';

const source=readFileSync(new URL('../macos/Sources/DesktopBridge/bridge.js',import.meta.url),'utf8');
const preflightSource=readFileSync(new URL('../macos/Sources/DesktopBridge/preflight.js',import.meta.url),'utf8');
function jwt(user:string,generation=1):string {
    return `header.${Buffer.from(JSON.stringify({exp:2000000000,generation,'https://api.openai.com/auth':{chatgpt_user_id:user,chatgpt_account_id:`workspace-${user}`}})).toString('base64url')}.synthetic`;
}
function fixture() {
    let token=jwt('a'),mode='chatgpt',active=false,success=true,readFails=false,clockOffset=0,completionDelay=0,threadsResumed:boolean|undefined;
    // Latest turn status of a thread in systemError; null when the thread is not in that state.
    let errored:string|null=null;
    let phase='idle',uiAvailable=true,uiFails=false,uiBegins=0,uiReconnects=0,uiFailures=0,uiChanged:unknown=null;
    const listeners:((e:{data:any})=>void)[]=[], replies:any[]=[];
    let injections=0;
    const emit=(data:any)=>listeners.forEach(l=>l({data}));
    const notify=(method:string,params:any)=>emit({type:'mcp-notification',hostId:'local',method,params});
    const send=async(message:any):Promise<void>=>{
        if(message.type==='mcp-response') {
            replies.push(message.response);
            if(message.response.result)token=message.response.result.accessToken;
            return;
        }
        const {id,method,params}=message.request;
        let result:any;
        switch(method) {
            case 'getAuthStatus': result={authMethod:mode,authToken:token};break;
            case 'thread/loaded/list':result={data:['synthetic-thread'],nextCursor:null};break;
            case 'thread/read':result={thread:{name:'合成任务',status:{type:active?'active':errored?'systemError':'idle',activeFlags:[]}}};break;
            case 'thread/turns/list':assert.equal(params.limit,1);result={data:[{id:'synthetic-turn',status:errored??'completed',items:[]}],nextCursor:null};break;
            case 'account/login/start':token=params.accessToken;mode='chatgptAuthTokens';injections++;result={type:mode};break;
            case 'account/read':result={account:{type:'chatgpt'}};break;
            default:throw Error('unexpected fixture method');
        }
        queueMicrotask(()=>{
            emit({type:'mcp-response',hostId:'local',message:{id,...readFails&&method==='account/read'?{error:{code:-32603,message:'synthetic-private-error'}}:{result}}});
            if(method==='account/login/start') {
                // Codex reads the account online before it reports completion.
                const complete=()=>{notify('account/login/completed',{loginId:null,success});if(success)notify('account/updated',{authMode:mode});};
                if(completionDelay)setTimeout(complete,completionDelay);else complete();
            }
        });
    };
    class Clock extends Date {static now():number{return Date.now()+clockOffset;}}
    const context=vm.createContext({window:{electronBridge:{sendMessageFromView:send},addEventListener:(_:string,l:(e:any)=>void)=>listeners.push(l),removeEventListener:(_:string,l:(e:any)=>void)=>{const i=listeners.indexOf(l);if(i>=0)listeners.splice(i,1)}},crypto:webcrypto,TextEncoder,TextDecoder,Uint8Array,atob,Date:Clock,setTimeout,clearTimeout});
    context.__authManagerSelectiveUIV1={ready:()=>uiAvailable,state:()=>({phase}),begin:async()=>{uiBegins++;phase='switching'},reconnect:async()=>{uiReconnects++;if(uiFails)throw Error('ui_services_unavailable');phase='idle';return{contextPreserved:threadsResumed!==false,...threadsResumed===undefined?{}:{threadsResumed}}},failed:(changed:unknown)=>{uiFailures++;uiChanged=changed;phase='idle'}};
    vm.runInContext(source,context);
    const call=(operation:string,params:any={}):Promise<any>=>operation==='preflight'?vm.runInContext(preflightSource,context):context.__authManagerDesktopV1.call({operation,params});
    const auth=(user:string,generation=1)=>({accessToken:jwt(user,generation),chatgptAccountId:`workspace-${user}`,chatgptPlanType:'pro'});
    const apply=(user:string)=>call('apply',{previousOwner:'codex:a:workspace-a',owner:`codex:${user}:workspace-${user}`,auth:auth(user)});
    const refresh=(id:number,workspace='workspace-b')=>emit({type:'mcp-request',hostId:'local',request:{id,method:'account/chatgptAuthTokens/refresh',params:{reason:'unauthorized',previousAccountId:workspace}}});
    return {call,auth,apply,refresh,notify,replies,bridge:()=>context.__authManagerDesktopV1,listenerCount:()=>listeners.length,state:()=>({token,mode,injections,phase,uiBegins,uiReconnects,uiFailures,uiChanged}),setUIAvailable:(v:boolean)=>{uiAvailable=v},setUIFails:(v:boolean)=>{uiFails=v},setActive:(v:boolean)=>{active=v},setErrored:(v:string|null)=>{errored=v},setSuccess:(v:boolean)=>{success=v},setReadFails:(v:boolean)=>{readFails=v},advance:(ms:number)=>{clockOffset+=ms},setCompletionDelay:(ms:number)=>{completionDelay=ms},setThreadsResumed:(v:boolean)=>{threadsResumed=v}};
}
let passed=0;
async function test(name:string,run:()=>Promise<void>):Promise<void>{await run();passed++;console.log(`PASS ${name}`);}
await test('preflight reads identity and idle status without injecting credentials',async()=>{
    const f=fixture();
    const bridge=f.bridge(), listeners=f.listenerCount();
    assert.equal((await f.call('preflight')).mode,'chatgpt');
    assert.equal(f.bridge(),bridge);
    assert.equal(f.listenerCount(),listeners);
    assert.equal(f.state().injections,0);
    assert.equal(f.state().token,jwt('a'));
    f.setActive(true);
    const refused=await f.call('preflight');
    assert.equal(refused.code,'tasks_active');
    assert.equal(JSON.stringify(refused.tasks),JSON.stringify([{name:'合成任务',status:'active',flags:[]}]));
    assert.equal(f.bridge(),bridge);
    assert.equal(f.listenerCount(),listeners);
    assert.equal(f.state().injections,0);
    assert.equal(f.state().mode,'chatgpt');
});
await test('systemError blocks preflight and apply only while the latest turn is still in progress',async()=>{
    const f=fixture();
    f.setErrored('failed');
    assert.equal((await f.call('preflight')).mode,'chatgpt');
    f.setErrored('inProgress');
    const refused=await f.call('preflight');
    assert.equal(refused.code,'tasks_active');
    assert.equal(JSON.stringify(refused.tasks),JSON.stringify([{name:'合成任务',status:'systemError',flags:[]}]));
    assert.equal((await f.apply('b')).code,'tasks_active');
    assert.equal(f.state().injections,0);
    f.setErrored('interrupted');
    assert.equal((await f.apply('b')).ok,true);
    assert.equal(f.state().injections,1);
});
await test('same process switch and account confirmation',async()=>{
    const f=fixture();assert.equal((await f.apply('b')).confirmed,true);
    const result=await f.call('apply',{previousOwner:'codex:b:workspace-b',owner:'codex:a:workspace-a',auth:f.auth('a')});
    assert.equal(result.confirmed,true);assert.equal(f.state().injections,2);
    assert.ok(!JSON.stringify(result).includes('accessToken'));
});
await test('unsupported UI adapter rejects before changing authentication',async()=>{
    const f=fixture();f.setUIAvailable(false);
    assert.equal((await f.apply('b')).code,'ui_adapter_unavailable');
    assert.equal(f.state().injections,0);assert.equal(f.state().uiBegins,0);
});
await test('same-account adoption and token refresh do not rebuild UI services',async()=>{
    const f=fixture();assert.equal((await f.apply('a')).confirmed,true);
    assert.equal((await f.call('update',{previousOwner:'codex:a:workspace-a',owner:'codex:a:workspace-a',auth:f.auth('a',2)})).confirmed,true);
    assert.equal(f.state().uiBegins,0);assert.equal(f.state().uiReconnects,0);
});
await test('a UI failure after the login hands the page back and leaves only a bind to finish',async()=>{
    const f=fixture();f.setUIFails(true);
    assert.equal((await f.apply('b')).code,'ui_services_unavailable');
    assert.equal(f.state().uiFailures,1);assert.equal(f.state().injections,1);
    assert.equal(f.state().uiChanged,true,'the app-server holds b, so the page is handed back for a rebuild');
    // The desktop is on b's login: the service binds it instead of logging in again.
    const bound=await f.call('bind',{owner:'codex:b:workspace-b'});
    assert.equal(bound.confirmed,true);assert.equal(bound.boundOwner,'codex:b:workspace-b');
    assert.equal(f.state().injections,1);assert.equal(f.state().uiBegins,1);
});
await test('active task blocks injection, including empty active flags',async()=>{
    const f=fixture();f.setActive(true);assert.equal((await f.apply('b')).code,'tasks_active');assert.equal(f.state().injections,0);
});
await test('a login Codex completes after 2.5 seconds is still confirmed',async()=>{
    // Ten real switches took 0.9–1.95 s from the ack to completion; the old 2 s wait had no margin.
    const f=fixture();f.setCompletionDelay(2500);
    const result=await f.apply('b');
    assert.equal(result.confirmed,true);assert.equal(result.threadsResumed,true);assert.equal(f.state().phase,'idle');
});
await test('an open task that did not resume is reported, not failed',async()=>{
    const f=fixture();f.setThreadsResumed(false);
    const result=await f.apply('b');
    assert.equal(result.confirmed,true);assert.equal(result.contextPreserved,false);assert.equal(result.threadsResumed,false);
});
await test('ack and changed memory cannot hide failed completion notification',async()=>{
    // Codex keeps the tokens when its online account check fails (success:false, no account/updated):
    // the page must be rebuilt on b even though its own account state never changed.
    const f=fixture();f.setSuccess(false);const r=await f.apply('b');assert.equal(r.confirmed,false);assert.equal(r.owner,'codex:b:workspace-b');
    assert.equal(f.state().uiFailures,1);assert.equal(f.state().uiChanged,true);
});
await test('post-install account read failure is a partial operation',async()=>{
    const f=fixture();f.setReadFails(true);const r=await f.apply('b');assert.equal(r.ok,false);assert.equal(f.state().token,jwt('b'));assert.ok(!JSON.stringify(r).includes('synthetic-private-error'));
    assert.equal(f.state().uiChanged,true);
});
await test('a refusal before any login hands the page back unchanged',async()=>{
    const f=fixture();f.setUIAvailable(false);const r=await f.apply('b');assert.equal(r.code,'ui_adapter_unavailable');
    assert.equal(f.state().injections,0);assert.equal(f.state().uiChanged,false);
});
await test('unrelated identity never injected',async()=>{
    const f=fixture();const r=await f.call('apply',{previousOwner:'other',owner:'codex:b:workspace-b',auth:f.auth('b')});assert.equal(r.code,'identity_changed');assert.equal(f.state().injections,0);
});
await test('refresh pins epoch, instance, identity; switch blocked until reply commits',async()=>{
    const f=fixture();await f.apply('b');f.refresh(100);let p=await f.call('poll');const request=p.requests[0];
    assert.equal((await f.call('apply',{previousOwner:'codex:b:workspace-b',owner:'codex:a:workspace-a',auth:f.auth('a')})).code,'refresh_pending');
    assert.equal((await f.call('reply',{...request,instance:'retired',auth:f.auth('b',2)})).code,'stale_refresh');
    assert.equal((await f.call('reply',{...request,instance:p.instance,auth:f.auth('a')})).code,'identity_changed');
    assert.equal(f.replies.length,0);
    assert.equal((await f.call('reply',{...request,instance:p.instance,auth:f.auth('b',2)})).ok,true);
    assert.equal(f.replies.length,1);assert.equal((await f.call('poll')).requests.length,0);
});
await test('expired or mismatched workspace request gets no new token',async()=>{
    const f=fixture();await f.apply('b');f.refresh(100,'workspace-a');await Promise.resolve();assert.equal(f.replies[0].result,undefined);
    f.refresh(101);const p=await f.call('poll');f.advance(9000);
    assert.equal((await f.call('reply',{...p.requests[0],instance:p.instance,auth:f.auth('b',2)})).code,'stale_refresh');
    await f.call('poll');assert.equal(f.replies.at(-1).result,undefined);
});
await test('same account token update works during an active task; identity change rejected',async()=>{
    const f=fixture();await f.apply('b');f.setActive(true);
    assert.equal((await f.call('update',{previousOwner:'codex:b:workspace-b',owner:'codex:b:workspace-b',auth:f.auth('b',2)})).confirmed,true);
    assert.equal((await f.call('update',{previousOwner:'codex:b:workspace-b',owner:'codex:a:workspace-a',auth:f.auth('a')})).code,'identity_changed');
});
await test('release rejects pending refresh and ends ownership',async()=>{
    const f=fixture();await f.apply('b');f.refresh(100);await f.call('release');assert.equal(f.replies[0].result,undefined);f.refresh(101);assert.equal((await f.call('poll')).requests.length,0);
});
await test('reopening manager explicitly rebinds external auth callbacks',async()=>{
    const f=fixture();await f.apply('b');await f.call('release');
    assert.equal((await f.call('poll')).boundOwner,null);
    assert.equal((await f.call('bind',{owner:'codex:a:workspace-a'})).ok,false);
    assert.equal((await f.call('bind',{owner:'codex:b:workspace-b'})).confirmed,true);
    f.refresh(200);const p=await f.call('poll');assert.equal(p.requests.length,1);assert.equal(p.boundOwner,'codex:b:workspace-b');
});
await test('quota notifications use core windows and are discarded when identity changes',async()=>{
    const f=fixture();await f.call('poll');
    const expiry=Math.floor(Date.now()/1000)+3000;
    f.notify('account/rateLimits/updated',{rateLimits:{limitId:'review',primary:{usedPercent:100,resetsAt:expiry}}});
    assert.equal((await f.call('poll')).usageObservation,null);
    f.notify('account/rateLimits/updated',{rateLimitsByLimitId:{codex:{limitId:'codex',primary:{usedPercent:98,resetsAt:expiry},secondary:{usedPercent:20,resetsAt:expiry+300}}}});
    const observation=(await f.call('poll')).usageObservation;
    assert.equal(observation.remaining,2);assert.equal(observation.expires_at,expiry);
    assert.equal(observation.owner,'codex:a:workspace-a');
    f.notify('account/updated',{authMode:'chatgpt'});
    assert.equal((await f.call('poll')).usageObservation,null);
    f.notify('account/rateLimits/updated',{rateLimits:{primary:{usedPercent:101,resetsAt:expiry}}});
    assert.equal((await f.call('poll')).usageObservation,null);
    assert.equal(f.state().injections,0);
});
console.log(JSON.stringify({passed,realCredentials:0,realDesktopMutations:0}));
