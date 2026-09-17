// Node execution of the browser-profile module with the same host and probe
// that the generated page runs in a real browser. Usage: node tests/browser.mjs module.wasm [--heap]
import { readFile } from 'node:fs/promises';
import { runProbe } from '../host/probe.mjs';

const [path, flag] = process.argv.slice(2);
if (!path) throw new Error('usage: node tests/browser.mjs module.wasm [--heap]');
const heap = flag === '--heap';
const result = await runProbe(await readFile(path), { heap });
console.log(`BROWSER-PROFILE NODE PASS ${path}: ${JSON.stringify(result)}`);
console.log(heap
    ? 'memory.grow-backed storage, exact import allowlist, injected host failures, sanitized outputs'
    : 'bounded arena without growth, exact import allowlist, injected host failures, sanitized outputs');
