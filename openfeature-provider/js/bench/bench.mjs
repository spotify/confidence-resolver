import process from 'node:process';
import { setTimeout as sleep, setImmediate } from 'node:timers/promises';
import { OpenFeature } from '@openfeature/server-sdk';
import { createConfidenceServerProvider } from '../dist/index.node.mjs';

function parseArgs(argv) {
  const args = {};
  for (let i = 2; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith('-')) continue;
    const [k, v] = a.split('=');
    const key = k.slice(1);
    if (v !== undefined) {
      args[key] = v;
    } else if (i + 1 < argv.length && !argv[i + 1].startsWith('-')) {
      args[key] = argv[++i];
    } else {
      args[key] = 'true';
    }
  }
  return args;
}

const args = parseArgs(process.argv);

const MOCK_HTTP = args['mock-http'] ?? process.env.MOCK_HTTP ?? 'http://localhost:8081';
const DURATION = Number(args['duration'] ?? process.env.DURATION ?? 30); // seconds
const WARMUP = Number(args['warmup'] ?? process.env.WARMUP ?? 5); // seconds
const FLAG_KEY = args['flag'] ?? process.env.FLAG_KEY ?? 'tutorial-feature';
const CLIENT_SECRET = args['client-secret'] ?? process.env.CONFIDENCE_FLAG_CLIENT_SECRET ?? 'secret';

// Rewrite all provider HTTP requests to the mock server (grpc-gateway and /state)
const proxyFetch = (input, init) => {
  const origUrl = new URL(input);
  const target = new URL(MOCK_HTTP);
  const newUrl = `${target.origin}${origUrl.pathname}`;
  const headers = new Headers(init?.headers);
  headers.set('X-Forwarded-Host', origUrl.host);
  return fetch(newUrl, { ...init, headers });
};

const provider = createConfidenceServerProvider({
  flagClientSecret: CLIENT_SECRET,
  flushInterval: 1000,
  fetch: proxyFetch,
});

// evaluation context similar to Go benchmark
const evalContext = { targetingKey: 'tutorial_visitor', visitor_id: 'tutorial_visitor' };

let completed = 0;
let errors = 0;

function runLoop(abort, client, flagKey, onErrorAbort) {
  return (async () => {
    let i = 0;
    while (!abort.aborted) {
      try {
        const details = await client.getObjectDetails(flagKey, {}, evalContext);
        completed++;
        if (details.reason === 'ERROR') {
          errors++;
          console.log(details);
          throw new Error(details.error);
        }
      } catch (e) {
        errors++;
        if (onErrorAbort) throw e;
      }
      // Occasionally yield to the event loop so timers/signals can fire even if resolves are synchronous
      if ((++i & 1023) === 0) {
        await setImmediate();
      }
    }
  })();
}

function seconds(n) {
  return n * 1000;
}

try {
  // setInterval(() => {
  //   console.log(process.memoryUsage());
  // }, 1000).unref();

  await OpenFeature.setProviderAndWait(provider);
  const client = OpenFeature.getClient();

  // Warmup
  if (WARMUP > 0) {
    const warmAbort = new AbortController();
    const warm = runLoop(warmAbort.signal, client, FLAG_KEY, true);
    await sleep(seconds(WARMUP));
    warmAbort.abort();
    await warm;
    if (errors > 0) {
      console.error('aborting: error during warmup');
      process.exit(1);
    }
  }
  console.log('starting');

  // Measurement with signal handling
  const measureAbort = new AbortController();
  const onSignal = () => measureAbort.abort();
  process.on('SIGINT', onSignal);
  process.on('SIGTERM', onSignal);

  const start = Date.now();
  const run = runLoop(measureAbort.signal, client, FLAG_KEY, true);
  await sleep(seconds(DURATION), undefined, { signal: measureAbort.signal });
  measureAbort.abort();
  await run;

  const elapsedMs = Date.now() - start;
  const qps = completed / (elapsedMs / 1000);
  console.log(
    `flag=${FLAG_KEY} duration=${Math.round(elapsedMs)}ms ops=${completed} errors=${errors} throughput=${Math.round(
      qps,
    )} ops/s`,
  );
} catch (err) {
  console.error(err);
  process.exit(1);
} finally {
  try {
    await provider.onClose();
  } catch {}
}
