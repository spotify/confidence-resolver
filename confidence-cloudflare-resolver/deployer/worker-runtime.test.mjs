// Local workerd regression for flags:apply failure diagnostics.
//
// Runs the *built* Worker (build/worker/shim.mjs + index.wasm, produced by
// `worker-build --release` in the parent directory) inside Miniflare/workerd
// with FLAG_LOG_SINK=buffer and drives it through its normal HTTP interface.
// All outbound traffic is intercepted locally: nothing is deployed and no
// production endpoint is contacted. Flag-log deliveries the buffer sink would
// POST to Confidence are recorded instead, so the test can tell whether an
// apply produced assignment events.
//
// Not part of `npm test` (the Docker image runs that before the Worker is
// built). Run with `npm run test:worker-runtime` after building. The Worker
// embeds ../../data/resolver_state_current.pb as a ClientResolverState, as
// the deployer produces it; the checked-in fixture is a bare ResolverState,
// so wrap it (state = field 1, account = field 2 from data/account_id) for
// the build. The tests skip when no build is present.
//
// This verifies plumbing (one structured console entry per failed apply, no
// change to the HTTP response) — not the cause of any historical failure.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile, access } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomBytes } from 'node:crypto';

const root = resolve(fileURLToPath(import.meta.url), '../..');
const workerDir = join(root, 'build/worker');
const CLIENT_SECRET = 'mkjJruAATQWjeY7foFIWfVAcBWnci2YF'; // test state fixture
const ORIGIN = 'https://app.example';

const built = await access(join(workerDir, 'index.wasm')).then(() => true, () => false);

const KEY = randomBytes(16);

async function startWorker() {
  const { Miniflare, Log, LogLevel } = await import('miniflare');
  const { parse } = await import('smol-toml');
  const config = parse(await readFile(join(root, 'wrangler.toml'), 'utf8'));
  const logs = [];
  const deliveries = [];
  const collect = (stream) => {
    let pending = '';
    stream.on('data', (chunk) => {
      pending += chunk;
      const lines = pending.split('\n');
      pending = lines.pop();
      logs.push(...lines);
    });
  };
  const mf = new Miniflare({
    host: '127.0.0.1', port: 0,
    modulesRoot: root,
    compatibilityDate: config.compatibility_date,
    compatibilityFlags: config.compatibility_flags ?? [],
    log: new Log(LogLevel.NONE),
    handleRuntimeStdio: (stdout, stderr) => { collect(stdout); collect(stderr); },
    modules: [
      { type: 'ESModule', path: join(workerDir, 'shim.mjs'),
        contents: await readFile(join(workerDir, 'shim.mjs'), 'utf8') },
      { type: 'CompiledWasm', path: join(workerDir, 'index.wasm'),
        contents: await readFile(join(workerDir, 'index.wasm')) },
    ],
    bindings: {
      CONFIDENCE_CLIENT_SECRET: CLIENT_SECRET,
      RESOLVE_TOKEN_ENCRYPTION_KEY: KEY.toString('base64'),
      FLAG_LOG_SINK: 'buffer',
      FORCE_APPLY: 'false',
      ALLOWED_ORIGIN: ORIGIN,
    },
    outboundService: async (request) => {
      if (new URL(request.url).pathname.endsWith('FlagLogs:write')) {
        // Edge destination (the fixture's default) receives JSON.
        const body = await request.json();
        deliveries.push((body.flagAssigned ?? []).length);
      }
      return new Response('{}', { status: 200 });
    },
  });
  await mf.ready;
  const post = (path, body) => mf.dispatchFetch(`http://worker.local/v1/${path}`, {
    method: 'POST', headers: { 'Content-Type': 'application/json', Origin: ORIGIN },
    body: JSON.stringify(body),
  });
  return { mf, logs, deliveries, post };
}

const diagnostics = (logs) => logs.filter((l) => l.includes('"event":"flag_apply_failed"'));
// The buffer sink flushes after 1s idle; wait well past that.
const assigned = (deliveries) => deliveries.reduce((a, b) => a + b, 0);
const settle = () => new Promise((r) => setTimeout(r, 3000));

async function resolveWithoutApply(post) {
  const res = await post('flags:resolve', {
    flags: ['flags/tutorial-feature'], clientSecret: CLIENT_SECRET, apply: false,
    evaluationContext: { visitor_id: 'tutorial_visitor' },
  });
  assert.equal(res.status, 200);
  const body = await res.json();
  assert.ok(body.resolveToken, 'resolve returned a token');
  // Only flags the SDK would apply are carried in the token.
  const flags = body.resolvedFlags.filter((f) => f.shouldApply).map((f) => f.flag);
  assert.ok(flags.length > 0, 'resolve returned applicable flags');
  return { token: body.resolveToken, flags };
}

function applyBody(resolveToken, flags) {
  const now = new Date().toISOString();
  return { clientSecret: CLIENT_SECRET, resolveToken, sendTime: now,
    flags: flags.map((flag) => ({ flag, applyTime: now })) };
}

function parseDiagnostic(line) {
  return JSON.parse(line.slice(line.indexOf('{')));
}

// Each case: a nonempty but unusable apply request, the 500 body the Worker
// already returned before this change, and the expected diagnostic.
// token_not_v1 / token_decrypt_failed are covered by the native classifier
// tests only: crafting such a token from outside the Worker would mean
// reproducing its token crypto, which this test deliberately does not do.
const failures = [
  {
    name: 'malformed token (too short for an IV)',
    request: (flags) => applyBody('AAAA', flags),
    body: /^internal error \[[A-Za-z0-9_-]{8}\]$/,
    stage: 'apply', kind: 'internal', code: true,
  },
  {
    name: 'unknown client secret',
    request: (flags, token) => ({ ...applyBody(token, flags), clientSecret: 'canary-unknown-secret' }),
    body: /./,
    stage: 'resolver_lookup', kind: 'resolver_lookup_failed', code: false,
  },
];

for (const c of failures) {
  test(`failed apply emits exactly one diagnostic: ${c.name}`, { skip: !built && 'worker not built' }, async () => {
    const { mf, logs, deliveries, post } = await startWorker();
    try {
      const resolved = await resolveWithoutApply(post);
      const { flags } = resolved;
      const request = c.request(flags, resolved.token);
      const res = await post('flags:apply', request);
      assert.equal(res.status, 500);
      assert.equal(res.headers.get('Access-Control-Allow-Origin'), ORIGIN);
      assert.match(await res.text(), c.body);
      await settle();
      assert.equal(assigned(deliveries), 0, 'no assignment events delivered');
      const found = diagnostics(logs);
      assert.equal(found.length, 1, `expected one diagnostic, got:\n${logs.join('\n')}`);
      const entry = parseDiagnostic(found[0]);
      assert.equal(entry.operation, 'flags:apply');
      assert.equal(entry.stage, c.stage);
      assert.equal(entry.error_kind, c.kind);
      assert.equal(entry.http_status, 500);
      if (c.code) assert.match(entry.diagnostic_code, /^[A-Za-z0-9_-]{8}$/);
      else assert.equal(entry.diagnostic_code, undefined);
      for (const secret of [CLIENT_SECRET, request.clientSecret, request.resolveToken,
        KEY.toString('base64'), 'tutorial_visitor', ...flags]) {
        assert.ok(!found[0].includes(secret), 'diagnostic must not contain request data');
      }
    } finally {
      await mf.dispose();
    }
  });
}

test('successful deferred apply and empty-token apply emit no diagnostic', { skip: !built && 'worker not built' }, async () => {
  const { mf, logs, deliveries, post } = await startWorker();
  try {
    const resolved = await resolveWithoutApply(post);
    await settle();
    assert.equal(assigned(deliveries), 0, 'apply=false resolve logs no assignment');

    const empty = await post('flags:apply', applyBody('', ['flags/whatever']));
    assert.equal(empty.status, 200);
    assert.equal(empty.headers.get('Access-Control-Allow-Origin'), ORIGIN);
    await settle();
    assert.equal(assigned(deliveries), 0, 'empty-token apply logs no assignment');

    const { flags } = resolved;
    const ok = await post('flags:apply', applyBody(resolved.token, flags));
    assert.equal(ok.status, 200);
    // The success branch has never set CORS headers; that is pre-existing
    // behaviour this change leaves alone, so it is not asserted here.
    await settle();
    assert.equal(assigned(deliveries), flags.length, 'successful apply delivered its assignments');
    assert.equal(diagnostics(logs).length, 0, logs.join('\n'));
  } finally {
    await mf.dispose();
  }
});
