// Generate a self-contained page: the module is embedded as base64 and the host
// sources are inlined, so the page runs from file:// without fetch, CORS or
// module resolution. Two page kinds share this generator:
//   node page.mjs module.wasm out.html [--heap]   boundary probe (tests/browser_page.html)
//   node page.mjs --app BrowserApp.wasm out.html   managed application (tests/app_page.html)
import { readFile, writeFile } from 'node:fs/promises';
import { basename } from 'node:path';

const argv = process.argv.slice(2);
const app = argv.includes('--app');
const heap = argv.includes('--heap');
const [wasmPath, outPath] = argv.filter((a) => !a.startsWith('--'));
if (!wasmPath || !outPath) throw new Error('usage: node page.mjs [--app] module.wasm out.html [--heap]');
const root = new URL('./', import.meta.url);
function inline(source) {
    // Keep the code identical to the tested modules; only the ESM plumbing goes.
    return source.split('\n').filter((line) => !/^import\s.*from\s+'\.\/(host|wasi)\.mjs';$/.test(line))
        .map((line) => line.replace(/^export\s+(const|class|function|async function)\s/, '$1 ').replace(/^export\s*\{[^}]*\};?$/, ''))
        .join('\n');
}
async function source(name) {
    const text = inline(await readFile(new URL(`host/${name}.mjs`, root), 'utf8'));
    if (/^(import|export)\s/m.test(text)) throw new Error(`${name}.mjs still contains module syntax after inlining`);
    return text;
}
const template = await readFile(new URL(app ? 'tests/app_page.html' : 'tests/browser_page.html', root), 'utf8');
let page = template
    .replace('__VARIANT__', basename(wasmPath))
    .replace('__HOST_JS__', await source('host'))
    .replace('__WASM_BASE64__', (await readFile(wasmPath)).toString('base64'));
page = app
    ? page.replace('__WASI_JS__', await source('wasi')).replace('__APP_JS__', await source('app'))
    : page.replace('__PROBE_JS__', await source('probe')).replace('__HEAP__', heap ? 'true' : 'false');
await writeFile(outPath, page);
console.log(`generated ${outPath} from ${wasmPath}`);
