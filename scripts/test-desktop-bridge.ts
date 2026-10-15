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
// oldAdapter: phase of a page adapter left in the page by a release before 0.18.
function fixture(oldAdapter?:string) {
    let token=jwt('a'),mode='chatgpt',active=false,success=true,readFails=false,clockOffset=0,completionDelay=0,landed=true;
    // Latest turn status of a thread in systemError; null when the thread is not in that state.
    let errored:string|null=null;
    // How the thread was started: a background generation is ephemeral with its own source.
    let origin:{ephemeral?:boolean,threadSource?:string|null}={};
    let reloads=0,disposed=0;
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
            case 'thread/read':result={thread:{name:'合成任务',status:{type:active?'active':errored?'systemError':'idle',activeFlags:[]},...origin}};break;
            case 'thread/turns/list':assert.equal(params.limit,1);result={data:[{id:'synthetic-turn',status:errored??'completed',items:[]}],nextCursor:null};break;
            case 'account/login/start':injections++;if(landed){token=params.accessToken;mode='chatgptAuthTokens';}result={type:'chatgptAuthTokens'};break;
            case 'account/read':result={account:{type:'chatgpt'}};break;
            default:throw Error('unexpected fixture method');
        }
        queueMicrotask(()=>{
            emit({type:'mcp-response',hostId:'local',message:{id,...readFails&&method==='account/read'?{error:{code:-32603,message:'synthetic-private-error'}}:{result}}});
            if(method==='account/login/start') {
                // Codex reads the account online before it reports completion.
                const complete=()=>{notify('account/login/completed',{loginId:null,success:success&&landed});if(success&&landed)notify('account/updated',{authMode:mode});};
                if(completionDelay)setTimeout(complete,completionDelay);else complete();
            }
        });
    };
    class Clock extends Date {static now():number{return Date.now()+clockOffset;}}
    const context=vm.createContext({window:{electronBridge:{sendMessageFromView:send},addEventListener:(_:string,l:(e:any)=>void)=>listeners.push(l),removeEventListener:(_:string,l:(e:any)=>void)=>{const i=listeners.indexOf(l);if(i>=0)listeners.splice(i,1)}},location:{reload:()=>{reloads++}},crypto:webcrypto,TextEncoder,TextDecoder,Uint8Array,atob,Date:Clock,setTimeout,clearTimeout});
    if(oldAdapter)context.__authManagerSelectiveUIV1={revision:6,state:()=>({phase:oldAdapter}),dispose:()=>{disposed++}};
    vm.runInContext(source,context);
    const call=(operation:string,params:any={}):Promise<any>=>operation==='preflight'?vm.runInContext(preflightSource,context):context.__authManagerDesktopV1.call({operation,params});
    const auth=(user:string,generation=1)=>({accessToken:jwt(user,generation),chatgptAccountId:`workspace-${user}`,chatgptPlanType:'pro'});
    const apply=(user:string)=>call('apply',{previousOwner:'codex:a:workspace-a',owner:`codex:${user}:workspace-${user}`,auth:auth(user)});
    const refresh=(id:number,workspace='workspace-b')=>emit({type:'mcp-request',hostId:'local',request:{id,method:'account/chatgptAuthTokens/refresh',params:{reason:'unauthorized',previousAccountId:workspace}}});
    return {call,auth,apply,refresh,notify,replies,bridge:()=>context.__authManagerDesktopV1,listenerCount:()=>listeners.length,state:()=>({token,mode,injections,reloads,disposed,adapter:context.__authManagerSelectiveUIV1}),setActive:(v:boolean)=>{active=v},setOrigin:(v:{ephemeral?:boolean,threadSource?:string|null})=>{origin=v},setErrored:(v:string|null)=>{errored=v},setSuccess:(v:boolean)=>{success=v},setReadFails:(v:boolean)=>{readFails=v},advance:(ms:number)=>{clockOffset+=ms},setCompletionDelay:(ms:number)=>{completionDelay=ms},setLanded:(v:boolean)=>{landed=v}};
}
// A reload is scheduled 200 ms after the bridge answers.
const settled=()=>new Promise(resolve=>setTimeout(resolve,250));
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
    assert.equal(JSON.stringify(refused.tasks),JSON.stringify([{id:'synthetic-thread',name:'合成任务',status:'active',flags:[],source:null}]));
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
    assert.equal(JSON.stringify(refused.tasks),JSON.stringify([{id:'synthetic-thread',name:'合成任务',status:'systemError',flags:[],source:null}]));
    assert.equal((await f.apply('b')).code,'tasks_active');
    assert.equal(f.state().injections,0);
    f.setErrored('interrupted');
    assert.equal((await f.apply('b')).ok,true);
    assert.equal(f.state().injections,1);
});
await test('same process switch and account confirmation; Codex rebuilds its own page',async()=>{
    const f=fixture();assert.equal((await f.apply('b')).confirmed,true);
    const result=await f.call('apply',{previousOwner:'codex:b:workspace-b',owner:'codex:a:workspace-a',auth:f.auth('a')});
    assert.equal(result.confirmed,true);assert.equal(f.state().injections,2);
    assert.ok(!JSON.stringify(result).includes('accessToken'));
    await settled();assert.equal(f.state().reloads,0);
});
await test('same-account adoption and token refresh never reload the page',async()=>{
    const f=fixture();assert.equal((await f.apply('a')).confirmed,true);
    assert.equal((await f.call('update',{previousOwner:'codex:a:workspace-a',owner:'codex:a:workspace-a',auth:f.auth('a',2)})).confirmed,true);
    await settled();assert.equal(f.state().reloads,0);
});
await test('active task blocks injection, including empty active flags',async()=>{
    const f=fixture();f.setActive(true);assert.equal((await f.apply('b')).code,'tasks_active');assert.equal(f.state().injections,0);
});
await test("the desktop's own background generations never block a switch; a side chat still does",async()=>{
    // 2026-10-15: an ambient-suggestions thread refused two switches while the person had nothing running.
    for(const origin of [{ephemeral:true,threadSource:'ambient_suggestions'},{ephemeral:true,threadSource:'ambient_suggestion_safety'},{ephemeral:true,threadSource:'mcp_extension_host'},{ephemeral:true,threadSource:null},{ephemeral:true}]) {
        const f=fixture();f.setActive(true);f.setOrigin(origin);
        assert.equal((await f.call('preflight')).mode,'chatgpt',JSON.stringify(origin));
        assert.equal((await f.apply('b')).confirmed,true,JSON.stringify(origin));
    }
    // A side chat is ephemeral but carries `user`; an unknown source and a saved thread count as tasks.
    for(const origin of [{ephemeral:true,threadSource:'user'},{ephemeral:true,threadSource:'chatgpt_hidden'},{ephemeral:false,threadSource:'ambient_suggestions'},{threadSource:'automation'}]) {
        const f=fixture();f.setActive(true);f.setOrigin(origin);
        const refused=await f.call('preflight');
        assert.equal(refused.code,'tasks_active',JSON.stringify(origin));
        assert.equal(refused.tasks[0].source,origin.threadSource);
        assert.equal((await f.apply('b')).code,'tasks_active',JSON.stringify(origin));
        assert.equal(f.state().injections,0);
    }
});
await test('a login Codex completes after 2.5 seconds is still confirmed',async()=>{
    // Ten real switches took 0.9–1.95 s from the ack to completion; the old 2 s wait had no margin.
    const f=fixture();f.setCompletionDelay(2500);
    const result=await f.apply('b');
    assert.equal(result.confirmed,true);
    await settled();assert.equal(f.state().reloads,0);
});
await test('a login Codex kept without telling its page reloads the page and leaves only a bind',async()=>{
    // Codex keeps the tokens when its online account check fails (success:false, no account/updated):
    // its page still shows a, so it reloads onto the account the app-server holds.
    const f=fixture();f.setSuccess(false);const r=await f.apply('b');assert.equal(r.confirmed,false);assert.equal(r.owner,'codex:b:workspace-b');
    await settled();assert.equal(f.state().reloads,1);
    // The desktop is on b's login: the service binds it instead of logging in again.
    const bound=await f.call('bind',{owner:'codex:b:workspace-b'});
    assert.equal(bound.confirmed,true);assert.equal(bound.boundOwner,'codex:b:workspace-b');
    assert.equal(f.state().injections,1);
});
await test('a login that never landed leaves the page alone',async()=>{
    const f=fixture();f.setLanded(false);const r=await f.apply('b');assert.equal(r.confirmed,false);assert.equal(r.owner,'codex:a:workspace-a');
    await settled();assert.equal(f.state().reloads,0);
});
await test('post-install account read failure is a partial operation; the page follows account/updated',async()=>{
    const f=fixture();f.setReadFails(true);const r=await f.apply('b');assert.equal(r.ok,false);assert.equal(f.state().token,jwt('b'));assert.ok(!JSON.stringify(r).includes('synthetic-private-error'));
    // Codex announced the new account, so its page rebuilds by itself.
    await settled();assert.equal(f.state().reloads,0);
    const g=fixture();g.setReadFails(true);g.setSuccess(false);assert.equal((await g.apply('b')).ok,false);
    await settled();assert.equal(g.state().reloads,1);
});
await test('unrelated identity never injected, and the page is left alone',async()=>{
    const f=fixture();const r=await f.call('apply',{previousOwner:'other',owner:'codex:b:workspace-b',auth:f.auth('b')});assert.equal(r.code,'identity_changed');assert.equal(f.state().injections,0);
    await settled();assert.equal(f.state().reloads,0);
});
await test('a page adapter from an older release is removed; one still holding the page is reloaded',async()=>{
    const idle=fixture('idle');
    assert.equal(idle.state().disposed,1);assert.equal(idle.state().adapter,undefined);
    assert.equal((await idle.apply('b')).confirmed,true);
    await settled();assert.equal(idle.state().reloads,0);
    for(const phase of ['failed','switching']) {
        const held=fixture(phase);
        assert.equal(held.state().disposed,0);assert.equal(held.state().adapter,undefined);
        await settled();assert.equal(held.state().reloads,1,phase);
    }
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
