import {readFileSync} from 'node:fs';
export const schema = JSON.parse(readFileSync(new URL('./schema.json',import.meta.url),'utf8'));
const INVAL=28, FAULT=21, IO=29, NOSYS=52;
/** Reference WASIp1 provider. Underlying functions operate on the SAME memory.
 * The request validation is not a sandbox guarantee for Node's experimental WASI.
 * An engine must independently enforce resource, filesystem and host permissions.
 */
export function createWasiHost(wasiImport, getMemory) {
    const counts = new Map();
    function dispatch_v1(rawPointer) {
        const memory=getMemory();
        if (!(memory instanceof WebAssembly.Memory) || memory.buffer instanceof SharedArrayBuffer) return INVAL;
        if (!Number.isInteger(rawPointer)) return FAULT;
        const address=rawPointer>>>0;
        if (address%8 || address > memory.buffer.byteLength-schema.request_size) return FAULT;
        const view=new DataView(memory.buffer,address,schema.request_size);
        if (view.getUint32(0,true)!==schema.version || view.getUint32(4,true)!==schema.request_size) return INVAL;
        const opcode=view.getUint32(8,true), argc=view.getUint32(12,true);
        const operation=schema.operations[opcode];
        if (!operation || operation.opcode!==opcode || argc!==operation.arguments.length) return INVAL;
        const args=[];
        for (let i=0;i<schema.max_args;++i) {
            const value=view.getBigUint64(16+i*8,true);
            if (i>=argc) { if (value!==0n) return INVAL; continue; }
            const kind=operation.arguments[i];
            if (kind==='u' || kind==='p') {
                if (value>0xffffffffn) return INVAL;
                args.push(Number(value));
            } else if (kind==='I') args.push(BigInt.asIntN(64,value));
            else if (kind==='U') args.push(value);
            else return INVAL;
        }
        const target=wasiImport[operation.name];
        if (typeof target!=='function') return NOSYS;
        counts.set(operation.name,(counts.get(operation.name)??0)+1);
        // proc_exit's host control transfer must reach wasi.start; do not catch
        // and relabel host exceptions as a successful exit or successful I/O.
        const result=target(...args);
        return Number.isInteger(result) && result>=0 && result<=76 ? result : IO;
    }
    return {imports:{dotnet_pal_host:{dispatch_v1}}, counts};
}
export function assertIsolatedImports(module) {
    const imports=WebAssembly.Module.imports(module);
    if (imports.length!==1 || imports[0].module!=='dotnet_pal_host' || imports[0].name!=='dispatch_v1' || imports[0].kind!=='function')
        throw new Error('OS isolation failed: '+JSON.stringify(imports));
    return imports;
}
