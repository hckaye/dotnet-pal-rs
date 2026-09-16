import test from 'node:test';
import assert from 'node:assert/strict';
import { memoryLimits, auditMemory, auditImports } from '../integration/llvm-wasi/wasm_contract.mjs';
const header = [0,97,115,109,1,0,0,0];
function module(payload) { return Uint8Array.from([...header,5,payload.length,...payload]); }
test('bounded wasm32 memory and the actual profile maximum', () => {
  assert.deepEqual(memoryLimits(module([1,1,1,2])), {minPages:1,maxPages:2});
  assert.equal(auditMemory(module([1,1,1,128,16])).maxPages, 2048);
  assert.throws(() => auditMemory(module([1,1,1,2])));
});
test('reject absent, shared, unbounded, duplicate, truncated and overflow declarations', () => {
  for (const bytes of [header, [...header,5,9,1], [...header,5,3,1,1,128],
      module([1,0,1]), module([1,3,1,2]), module([2,1,1,2,1,1,2]),
      module([1,1,3,2]), module([1,1,255,255,255,255,31,2]),
      [...module([1,1,1,2]),5,4,1,1,1,2]]) assert.throws(() => memoryLimits(bytes));
});
test('reject unexpected WASI function, foreign module, imported memory and missing imports', () => {
  for (const entry of [
    {module:'wasi_snapshot_preview1',name:'sock_accept',kind:'function'},
    {module:'env',name:'clock_time_get',kind:'function'},
    {module:'wasi_snapshot_preview1',name:'clock_time_get',kind:'memory'}
  ]) assert.throws(() => auditImports([entry]));
  assert.throws(() => auditImports([]));
});
