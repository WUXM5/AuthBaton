import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {createHash} from 'node:crypto';
import {spawn,execFileSync} from 'node:child_process';
import {mkdtempSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {once} from 'node:events';
import type {Socket} from 'node:net';
import vm from 'node:vm';

const renderer=vm.createContext({});

const sockets=new Set<Socket>();let calls=0;
const temporary=mkdtempSync(join(tmpdir(),'authmanager-cdp-transport-'));
function frame(value:unknown):Buffer {
    const payload=Buffer.from(JSON.stringify(value));
    assert.ok(payload.length<65536);
    const h=Buffer.alloc(payload.length<126?2:4);h[0]=0x81;
    if(h.length===2)h[1]=payload.length;else{h[1]=126;h.writeUInt16BE(payload.length,2);}
    return Buffer.concat([h,payload]);
}
const server=createServer();
server.on('upgrade',(req,socket)=>{
    const s=socket as Socket;sockets.add(s);s.on('close',()=>sockets.delete(s));
    const key=createHash('sha1').update(String(req.headers['sec-websocket-key'])+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
    s.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${key}\r\n\r\n`);
    let buffer=Buffer.alloc(0);
    s.on('data',chunk=>{
        buffer=Buffer.concat([buffer,chunk]);
        while(buffer.length>=6){
            const opcode=buffer[0]&15;let size=buffer[1]&127,offset=2;
            if(size===126){if(buffer.length<4)return;size=buffer.readUInt16BE(2);offset=4;}
            assert.notEqual(size,127);assert.ok(buffer[1]&128);
            if(buffer.length<offset+4+size)return;
            const mask=buffer.subarray(offset,offset+4),payload=Buffer.from(buffer.subarray(offset+4,offset+4+size));
            for(let i=0;i<payload.length;i++)payload[i]^=mask[i%4];
            buffer=buffer.subarray(offset+4+size);
            if(opcode===8){s.end(Buffer.from([0x88,0]));return;}
            assert.equal(opcode,1,'CDP must use text frames');
            const message=JSON.parse(payload.toString());calls++;
            s.write(frame({method:'Runtime.executionContextCreated',params:{context:{id:1}}}));
            const evaluated=message.params?.expression?.startsWith('// Desktop-only')
                ? vm.runInContext(message.params.expression,renderer) : {fixture:true};
            s.write(frame(message.method==='synthetic/error'
                ?{id:message.id,error:{code:-32603,message:'SYNTHETIC_PRIVATE_ERROR'}}
                :{id:message.id,result:{result:evaluated===undefined?{type:'undefined'}:{value:evaluated}}}));
        }
    });
});
server.listen(0,'127.0.0.1');await once(server,'listening');
try {
    const address=server.address();assert.ok(address&&typeof address==='object');
    const binary=join(temporary,'transport-smoke');
    execFileSync('xcrun',['swiftc','-parse-as-library','-swift-version','6','-sdk','/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk','macos/Sources/DesktopBridge/CDP.swift','scripts/desktop-transport-smoke.swift','-o',binary],{stdio:'inherit'});
    const child=spawn(binary,[String(address.port),'macos/Sources/DesktopBridge/automatic.js'],{stdio:'inherit'});
    const [code]=await once(child,'exit');assert.equal(code,0);assert.equal(calls,4);
    console.log(JSON.stringify({cdpTextRequests:calls,realDesktopMutations:0}));
} finally {for(const socket of sockets)socket.destroy();server.close();rmSync(temporary,{recursive:true,force:true});}
