// Node execution of the managed BrowserApp with the page hosts.
// Usage: node tests/app.mjs artifacts/app/BrowserApp.wasm
import { readFile } from 'node:fs/promises';
import { runApp, assertAppPassed } from '../host/app.mjs';

const [path] = process.argv.slice(2);
if (!path) throw new Error('usage: node tests/app.mjs BrowserApp.wasm');
const result = await runApp(await readFile(path));
for (const line of result.lines) console.log('  ' + line);
const pass = assertAppPassed(result);
console.log(`MANAGED BROWSER NODE PASS ${path}: ${pass}`);
console.log(`pal=${JSON.stringify(result.palCalls)} wasi=${JSON.stringify(result.wasiCalls)} memory=${result.initialMemory}->${result.finalMemory}`);
