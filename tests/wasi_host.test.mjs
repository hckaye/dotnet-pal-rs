import test from 'node:test';
import assert from 'node:assert/strict';
import {schema,createWasiHost} from '../integration/wasi/host.mjs';
const memory=new WebAssembly.Memory({initial:1});
function request(op, args=[]) {
    const v=new DataView(memory.buffer,128,88);
    new Uint8Array(memory.buffer,128,88).fill(0);
    v.setUint32(0,1,true);v.setUint32(4,88,true);v.setUint32(8,op,true);v.setUint32(12,args.length,true);
    args.forEach((x,i)=>v.setBigUint64(16+i*8,BigInt.asUintN(64,BigInt(x)),true));
    return v;
}
test('all 45 physical import signatures preserve width and signedness',()=>{
    for(const op of schema.operations) {
        const slots=[...op.arguments].map((k,i)=>k==='I'?-3n:k==='U'?(1n<<62n)+BigInt(i):BigInt(1024+i));
        let hits=0;
        const host=createWasiHost({[op.name](...args){
            ++hits;
            assert.deepEqual(args,slots.map((x,i)=>'UI'.includes(op.arguments[i])?x:Number(x)));
            return 0;
        }},()=>memory);
        request(op.opcode,slots);
        assert.equal(host.imports.dotnet_pal_host.dispatch_v1(128),0);
        assert.equal(hits,1); assert.equal(host.counts.get(op.name),1);
    }
});
test('malformed request geometry/header/arity/slot width is rejected without host calls',()=>{
    const op=schema.operations.find(x=>x.name==='random_get'); let hits=0;
    const host=createWasiHost({random_get(){++hits;return 0;}},()=>memory);
    const run=host.imports.dotnet_pal_host.dispatch_v1;
    request(op.opcode,[1024,16]);assert.equal(run(129),21);assert.equal(run(65520),21);
    let v=request(op.opcode,[1024,16]);v.setUint32(0,2,true);assert.equal(run(128),28);
    v=request(op.opcode,[1024,16]);v.setUint32(4,80,true);assert.equal(run(128),28);
    request(99,[]);assert.equal(run(128),28);
    request(op.opcode,[1024]);assert.equal(run(128),28);
    request(op.opcode,[1n<<32n,16]);assert.equal(run(128),28);
    v=request(op.opcode,[1024,16]);v.setBigUint64(80,1n,true);assert.equal(run(128),28);
    assert.equal(hits,0);
});
test('unknown status and missing service never become success; shared memory rejected',()=>{
    const op=schema.operations.find(x=>x.name==='sched_yield');request(op.opcode,[]);
    for(const result of [65536,-1,undefined,1.5]) {
        const host=createWasiHost({sched_yield:()=>result},()=>memory);
        assert.equal(host.imports.dotnet_pal_host.dispatch_v1(128),29);
    }
    assert.equal(createWasiHost({},()=>memory).imports.dotnet_pal_host.dispatch_v1(128),52);
    const shared=new WebAssembly.Memory({initial:1,maximum:1,shared:true});
    assert.equal(createWasiHost({},()=>shared).imports.dotnet_pal_host.dispatch_v1(128),28);
});
