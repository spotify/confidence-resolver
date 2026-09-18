import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const MIB = 1024 * 1024;
const script = fileURLToPath(import.meta.url);

const config = Object.freeze({ warnBytes: 80 * MIB, abortBytes: 96 * MIB, timeoutMs: 60_000 });

export function assess(measurement) {
  const { wasmBytes, jsHeapCapacityBytes, jsHeapUsedBytes } = measurement;
  for (const value of [wasmBytes, jsHeapCapacityBytes, jsHeapUsedBytes]) {
    if (!Number.isSafeInteger(value) || value <= 0) throw new Error('invalid_measurement');
  }
  if (jsHeapUsedBytes > jsHeapCapacityBytes) throw new Error('invalid_measurement');
  const baselineBytes = wasmBytes + jsHeapCapacityBytes;
  return {
    wasmBytes, jsHeapCapacityBytes, jsHeapUsedBytes, baselineBytes,
    warnBytes: config.warnBytes, abortBytes: config.abortBytes,
    status: baselineBytes >= config.abortBytes ? 'abort_deployment'
      : baselineBytes >= config.warnBytes ? 'warning' : 'pass',
  };
}

export function deploymentDecision(report, forceDeploy) {
  const forced = report.status === 'abort_deployment' && Boolean(forceDeploy);
  return { ...report, forced, exitCode: report.status === 'abort_deployment' && !forced ? 1 : 0 };
}

// Extra entrypoints/configs/environments or build transforms would deploy a
// different artifact than the one measured. Unknown arguments fail closed;
// FORCE_DEPLOY can explicitly override this signal too.
export function validateDeployArgs(args) {
  const values = new Set(['--tag', '--message', '--name', '--var', '--route']);
  const flags = new Set(['--keep-vars', '--dry-run', '--logpush', '--upload-source-maps', '--no-bundle', '--no-build']);
  for (let i = 0; i < args.length; i++) {
    const [name] = args[i].split('=', 1);
    if (values.has(name)) {
      if (!args[i].includes('=') && (++i >= args.length || args[i].startsWith('--'))) {
        throw new Error('unsupported_deploy_arguments');
      }
    } else if (!flags.has(args[i])) {
      throw new Error('unsupported_deploy_arguments');
    }
  }
}

// Run in a separate process group: a stuck/OOMing runtime must not hang deployment.
export async function runProbe(root, timeoutMs) {
  const directory = await mkdtemp(join(tmpdir(), 'resolver-memory-'));
  let child;
  let closed;
  let timer;
  try {
    return await new Promise((accept, reject) => {
      child = spawn(process.execPath, [script, '--measure', root], {
        cwd: directory, detached: true, stdio: ['ignore', 'pipe', 'ignore'],
        env: { PATH: process.env.PATH, TMPDIR: directory,
          MINIFLARE_WORKERD_PATH: process.env.MINIFLARE_WORKERD_PATH },
      });
      closed = new Promise((acceptClose) => child.once('close', acceptClose));
      let output = '';
      timer = setTimeout(() => {
        reject(new Error('measurement_timeout'));
      }, timeoutMs);
      child.stdout.on('data', (chunk) => {
        output += chunk;
        if (output.length > 65536) reject(new Error('invalid_measurement'));
      });
      child.on('error', () => { clearTimeout(timer); reject(new Error('measurement_failed')); });
      child.on('close', (code) => {
        clearTimeout(timer);
        if (code !== 0) return reject(new Error('measurement_failed'));
        try { accept(JSON.parse(output)); }
        catch { reject(new Error('invalid_measurement')); }
      });
    });
  } finally {
    clearTimeout(timer);
    // Also remove workerd descendants if the probe timed out or failed to dispose.
    if (child?.pid) {
      try { process.kill(-child.pid, 'SIGKILL'); }
      catch (error) { if (error.code !== 'ESRCH') throw error; }
    }
    await closed;
    await rm(directory, { recursive: true, force: true });
  }
}

export async function measure(root) {
  const { Miniflare, Log, LogLevel } = await import('miniflare');
  const { parse } = await import('smol-toml');
  const config = parse(await readFile(join(root, 'wrangler.toml'), 'utf8'));
  const workerDir = join(root, 'build/worker');
  if (resolve(root, config.main ?? '') !== join(workerDir, 'shim.mjs')) {
    throw new Error('unsupported_worker_entrypoint');
  }
  const mf = new Miniflare({
    host: '127.0.0.1', port: 0, inspectorPort: 0,
    modulesRoot: root,
    compatibilityDate: config.compatibility_date,
    compatibilityFlags: config.compatibility_flags ?? [],
    log: new Log(LogLevel.NONE),
    modules: [
      { type: 'ESModule', path: join(root, 'memory-probe.mjs'), contents: `
        import { resolver_memory_preflight } from './build/worker/shim.mjs';
        export default { fetch() {
          return Response.json({ wasmBytes: resolver_memory_preflight() });
        }};` },
      { type: 'ESModule', path: join(workerDir, 'shim.mjs'),
        contents: await readFile(join(workerDir, 'shim.mjs'), 'utf8') },
      { type: 'CompiledWasm', path: join(workerDir, 'index.wasm'),
        contents: await readFile(join(workerDir, 'index.wasm')) },
    ],
    outboundService: () => new Response(null, { status: 403 }),
  });
  let ws;
  try {
    await mf.ready;
    const listUrl = new URL('/json/list', await mf.getInspectorURL());
    listUrl.protocol = 'http:';
    const targets = await (await fetch(listUrl)).json();
    const target = targets.find((t) => t.id?.includes('core:user:'));
    if (!target) throw new Error('inspector_unavailable');
    ws = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((accept, reject) => {
      ws.addEventListener('open', accept, { once: true });
      ws.addEventListener('error', reject, { once: true });
    });
    const response = await mf.dispatchFetch('http://memory-preflight.local/');
    if (response.status !== 200) throw new Error('initialization_failed');
    const { wasmBytes } = await response.json();
    const heap = await new Promise((accept, reject) => {
      ws.addEventListener('message', ({ data }) => {
        const message = JSON.parse(data);
        if (message.id === 1) {
          if (message.error) reject(new Error('inspector_failed'));
          else accept(message.result);
        }
      });
      ws.send(JSON.stringify({ id: 1, method: 'Runtime.getHeapUsage' }));
    });
    return { wasmBytes, jsHeapCapacityBytes: heap.totalSize, jsHeapUsedBytes: heap.usedSize };
  } finally {
    ws?.close();
    await mf.dispose();
  }
}

async function main() {
  if (process.env.SKIP_PREFLIGHT_TEST === 'true') {
    console.log(JSON.stringify({ status: 'skipped', reason: 'SKIP_PREFLIGHT_TEST', exitCode: 0 }));
    console.error('!!! SKIP_PREFLIGHT_TEST=true: memory preflight skipped. Runtime memory has not been checked. !!!');
    return;
  }
  let report;
  try {
    validateDeployArgs(process.argv.slice(3));
    report = assess(await runProbe(resolve(process.argv[2] ?? '.'), config.timeoutMs));
  } catch (error) {
    // Never emit exception text from parsing customer data or runtime diagnostics.
    const reason = ['measurement_timeout', 'invalid_measurement', 'unsupported_deploy_arguments']
      .includes(error.message) ? error.message : 'measurement_failed';
    report = { status: 'abort_deployment', reason };
  }
  const decision = deploymentDecision(report, process.env.FORCE_DEPLOY);
  console.log(JSON.stringify(decision));
  if (report.baselineBytes !== undefined) {
    console.error(`Memory preflight: WASM ${(report.wasmBytes / MIB).toFixed(2)} MiB + JS capacity ${(report.jsHeapCapacityBytes / MIB).toFixed(2)} MiB = ${(report.baselineBytes / MIB).toFixed(2)} MiB baseline.`);
  }
  if (decision.forced) {
    console.error('!!! FORCE_DEPLOY OVERRIDE: MEMORY PREFLIGHT REQUESTED abort_deployment. Continuing at operator request. !!!');
  } else if (decision.exitCode) {
    console.error('!!! MEMORY PREFLIGHT: DEPLOYMENT BLOCKED. State exceeds the memory budget or could not be measured. !!!');
  } else if (report.status === 'warning') {
    console.error('!!! MEMORY PREFLIGHT WARNING: state-loaded baseline exceeds the warning budget. !!!');
  }
  console.error('Baseline only: request allocations and runtime overhead are additional; this is not exact Cloudflare quota accounting.');
  process.exitCode = decision.exitCode;
}

if (process.argv[1] && resolve(process.argv[1]) === script) {
  if (process.argv[2] === '--measure') {
    try { console.log(JSON.stringify(await measure(resolve(process.argv[3])))); }
    catch { process.exitCode = 1; }
  } else {
    await main();
  }
}
