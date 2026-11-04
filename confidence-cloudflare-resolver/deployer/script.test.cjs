const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const { existsSync, mkdtempSync, readFileSync, copyFileSync, rmSync, writeFileSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { test } = require('node:test');
const { parse: parseToml } = require('smol-toml');

const wranglerTomlPath = join(__dirname, '../wrangler.toml');

function runValidation(count) {
  const result = spawnSync('bash', ['-c', `
set -euo pipefail
FLAG_LOGS_QUEUE_COUNT=${count}
if [[ ! "$FLAG_LOGS_QUEUE_COUNT" =~ ^[1-9][0-9]*$ ]] || [ "$FLAG_LOGS_QUEUE_COUNT" -gt 9999 ]; then
    echo "FLAG_LOGS_QUEUE_COUNT must be a positive integer between 1 and 9999" >&2
    exit 1
fi
echo "OK"
`], { encoding: 'utf8', timeout: 5000 });
  return result;
}

function runWranglerAppend(count, prefix = '') {
  const directory = mkdtempSync(join(tmpdir(), 'flag-log-queues-test-'));
  try {
    copyFileSync(wranglerTomlPath, join(directory, 'wrangler.toml'));
    const result = spawnSync('bash', ['-c', `
set -euo pipefail
FLAG_LOGS_QUEUE_COUNT=${count}
WORKER_NAME_PREFIX="${prefix}"
BASE_QUEUE_NAME="flag-logs-queue"
EVENTS_QUEUE_NAME="events-queue"
if [ -n "$WORKER_NAME_PREFIX" ]; then
    BASE_QUEUE_NAME="\${WORKER_NAME_PREFIX}-flag-logs-queue"
    EVENTS_QUEUE_NAME="\${WORKER_NAME_PREFIX}-events-queue"
    WORKER_NAME="\${WORKER_NAME_PREFIX}-confidence-cloudflare-resolver"
    sed -i.tmp "s/^name = .*/name = \\"\$WORKER_NAME\\"/" wrangler.toml
    sed -i.tmp "s/queue = \\"flag-logs-queue\\"/queue = \\"\$BASE_QUEUE_NAME\\"/g" wrangler.toml
    sed -i.tmp "s/queue = \\"events-queue\\"/queue = \\"\$EVENTS_QUEUE_NAME\\"/g" wrangler.toml
fi
for ((queue_index = 2; queue_index <= FLAG_LOGS_QUEUE_COUNT; queue_index++)); do
    if [ -n "\$WORKER_NAME_PREFIX" ]; then
        SHARD_NAME="\${WORKER_NAME_PREFIX}-flag-logs-queue-\${queue_index}"
    else
        SHARD_NAME="flag-logs-queue-\${queue_index}"
    fi
    cat >> wrangler.toml <<EOF

[[queues.consumers]]
queue = "\${SHARD_NAME}"
max_batch_size = 100
max_batch_timeout = 10

[[queues.producers]]
queue = "\${SHARD_NAME}"
binding = "flag_logs_queue_\${queue_index}"
EOF
done
`], { cwd: directory, encoding: 'utf8', timeout: 5000 });
    const config = readFileSync(join(directory, 'wrangler.toml'), 'utf8');
    return { ...result, config };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function extractQueues(config) {
  const consumers = [...config.matchAll(/\[\[queues\.consumers\]\]\s+queue = "([^"]+)"/g)].map(m => m[1]);
  const producers = [...config.matchAll(/\[\[queues\.producers\]\]\s+queue = "([^"]+)"\s+binding = "([^"]+)"/g)].map(m => ({ queue: m[1], binding: m[2] }));
  return { consumers, producers };
}

// --- Validation tests ---

for (const count of ['0', '-1', '1.5', 'abc', '01', '10000']) {
  test(`rejects invalid count ${count}`, () => {
    const result = runValidation(count);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /FLAG_LOGS_QUEUE_COUNT must be/);
  });
}

for (const count of ['1', '2', '99', '9999']) {
  test(`accepts valid count ${count}`, () => {
    const result = runValidation(count);
    assert.equal(result.status, 0);
    assert.match(result.stdout, /OK/);
  });
}

// --- Wrangler.toml generation tests ---

test('count=1 no prefix: only base queue, no extra shards', () => {
  const { status, config } = runWranglerAppend(1);
  assert.equal(status, 0);
  const { consumers, producers } = extractQueues(config);
  assert.deepEqual(consumers, ['flag-logs-queue', 'events-queue']);
  assert.deepEqual(producers.map(p => p.binding), ['flag_logs_queue', 'events_queue']);
});

test('count=2 no prefix: base + shard 2', () => {
  const { status, config } = runWranglerAppend(2);
  assert.equal(status, 0);
  const { consumers, producers } = extractQueues(config);
  assert.deepEqual(consumers, ['flag-logs-queue', 'events-queue', 'flag-logs-queue-2']);
  const flagProducers = producers.filter(p => p.binding.startsWith('flag_logs_queue'));
  assert.deepEqual(flagProducers, [
    { queue: 'flag-logs-queue', binding: 'flag_logs_queue' },
    { queue: 'flag-logs-queue-2', binding: 'flag_logs_queue_2' },
  ]);
});

test('count=3 no prefix: base + shard 2 + shard 3', () => {
  const { status, config } = runWranglerAppend(3);
  assert.equal(status, 0);
  const { consumers } = extractQueues(config);
  assert.deepEqual(consumers, ['flag-logs-queue', 'events-queue', 'flag-logs-queue-2', 'flag-logs-queue-3']);
});

test('count=2 with prefix: names are prefixed', () => {
  const { status, config } = runWranglerAppend(2, 'customer');
  assert.equal(status, 0);
  const { consumers, producers } = extractQueues(config);
  assert.deepEqual(consumers, ['customer-flag-logs-queue', 'customer-events-queue', 'customer-flag-logs-queue-2']);
  const flagProducers = producers.filter(p => p.binding.startsWith('flag_logs_queue'));
  assert.deepEqual(flagProducers, [
    { queue: 'customer-flag-logs-queue', binding: 'flag_logs_queue' },
    { queue: 'customer-flag-logs-queue-2', binding: 'flag_logs_queue_2' },
  ]);
});

test('count=1 with prefix: only base queue prefixed', () => {
  const { status, config } = runWranglerAppend(1, 'customer');
  assert.equal(status, 0);
  const { consumers } = extractQueues(config);
  assert.deepEqual(consumers, ['customer-flag-logs-queue', 'customer-events-queue']);
});

// --- Flag-log sink tests ---

// Runs the real validation block out of script.sh rather than a copy, so
// this cannot silently pass while the script says something else.
const SINK_VALIDATION = (() => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const match = script.match(/^case "\$FLAG_LOG_SINK" in[\s\S]*?^esac$/m);
  assert.ok(match, 'could not find the FLAG_LOG_SINK validation in script.sh');
  return match[0];
})();

function runSinkValidation(value) {
  return spawnSync('bash', ['-c', `
set -euo pipefail
FLAG_LOG_SINK=${value === undefined ? '${FLAG_LOG_SINK:-queue}' : `"${value}"`}
FLAG_LOG_SINK=$(printf '%s' "$FLAG_LOG_SINK" | tr '[:upper:]' '[:lower:]')
${SINK_VALIDATION}
echo "$FLAG_LOG_SINK"
`], { encoding: 'utf8', timeout: 5000, env: { ...process.env, FLAG_LOG_SINK: '' } });
}

for (const value of ['queue', 'QUEUE', 'buffer', 'BUFFER']) {
  test(`accepts sink ${value} and lowercases it`, () => {
    const result = runSinkValidation(value);
    assert.equal(result.status, 0);
    assert.equal(result.stdout.trim(), value.toLowerCase());
  });
}

for (const value of ['', 'r2', 'logpush', 'true', 'queues', 'buffered', 'memory']) {
  test(`rejects sink ${JSON.stringify(value)}`, () => {
    const result = runSinkValidation(value);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /FLAG_LOG_SINK must be/);
  });
}

test('defaults to queue when unset', () => {
  const result = runSinkValidation(undefined);
  assert.equal(result.status, 0);
  assert.equal(result.stdout.trim(), 'queue');
});

test('buffer mode provisions no transport of its own', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const branch = script.match(/if \[ "\$FLAG_LOG_SINK" = "buffer" \][\s\S]*?^else$/m);
  assert.ok(branch, 'buffer branch not found');
  assert.doesNotMatch(branch[0], /logpush|r2\/buckets|curl/i,
    'buffer mode must not provision transport resources');
});

test('logpush is gone from the deployer entirely', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  assert.doesNotMatch(script, /logpush/i, 'no logpush leftovers');
  assert.doesNotMatch(script, /FLAG_LOGS_INGEST_TOKEN/, 'no ingest token leftovers');
});

// --- API token preflight tests ---

// Replicates the preflight probe loop with a stubbed `curl`, so the tests
// cover the branching and messaging without reaching the network. `codes`
// maps an endpoint fragment to the HTTP status the stub should return.
function runPreflight(sink, codes, extra = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'preflight-test-'));
  try {
    const cases = Object.entries(codes)
      .map(([frag, code]) => `  *${frag}*) echo -n "${code}";;`)
      .join('\n');
    writeFileSync(join(directory, 'curl'), `#!/bin/bash
for a in "$@"; do case "$a" in https://*) url="$a";; esac; done
case "$url" in
${cases}
  *) echo -n "200";;
esac
`, { mode: 0o755 });

    const result = spawnSync('bash', ['-c', `
set -uo pipefail
export PATH="${directory}:$PATH"
CLOUDFLARE_ACCOUNT_ID=acct123
CLOUDFLARE_API_TOKEN=tok
FLAG_LOG_SINK=${sink}
ENABLE_METRICS="${extra.metrics || ''}"
ENABLE_STICKY_ASSIGNMENTS=""
missing=0
base="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
check_perm() {
    label="$1"; perm="$2"; url="$3"
    code=$(curl -sS -o /dev/null -w "%{http_code}" -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$url")
    if [ "$code" = "200" ]; then echo "PROBE_OK ${'${label}'}"; else
        echo "PROBE_FAIL ${'${label}'} needs: ${'${perm}'}" >&2; missing=1; fi
}
check_perm "Workers Scripts" "Account > Workers Scripts > Edit" "$base/workers/scripts"
check_perm "Workers Queues"  "Account > Workers Queues > Edit"  "$base/queues"
if [ -n "$ENABLE_METRICS" ]; then
    check_perm "Workers KV" "Account > Workers KV Storage > Edit" "$base/storage/kv/namespaces"
fi
[ "$missing" -ne 0 ] && exit 1
exit 0
`], { encoding: 'utf8', timeout: 5000 });
    return result;
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test('preflight passes when every probe returns 200', () => {
  const r = runPreflight('queue', {});
  assert.equal(r.status, 0, r.stderr);
  assert.equal((r.stdout.match(/PROBE_OK/g) || []).length, 2);
});

// Neither sink needs anything beyond scripts and queues: the buffer
// provisions no transport at all, and queue bindings exist under both.
test('buffer mode probes nothing extra', () => {
  const r = runPreflight('buffer', {});
  assert.equal(r.status, 0);
  assert.equal((r.stdout.match(/PROBE_OK/g) || []).length, 2);
});

test('a 403 fails the deploy and names the scope', () => {
  const r = runPreflight('queue', { 'workers/scripts': 403 });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /PROBE_FAIL Workers Scripts needs: Account > Workers Scripts > Edit/);
});

// A token valid for one account returns an indistinguishable auth error for
// another, so a wrong-account token must fail here rather than part-way in.
test('a token with no access to the account fails every probe', () => {
  const r = runPreflight('queue', { 'workers/scripts': 403, queues: 403 });
  assert.equal(r.status, 1);
  assert.equal((r.stderr.match(/PROBE_FAIL/g) || []).length, 2);
});

test('KV is only probed when metrics are enabled', () => {
  assert.doesNotMatch(runPreflight('queue', {}).stdout, /Workers KV/);
  assert.match(runPreflight('queue', {}, { metrics: '1' }).stdout, /PROBE_OK Workers KV/);
});

// The real script probes KV for sticky assignments too; the harness above
// only models ENABLE_METRICS, so assert the script itself covers both.
test('the script probes KV for sticky assignments as well as metrics', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const guard = script.match(/if \[ -n "\$\{ENABLE_METRICS:-\}" \][^\n]*\n/);
  assert.ok(guard, 'KV probe guard not found');
  assert.match(
    guard[0],
    /ENABLE_STICKY_ASSIGNMENTS/,
    'KV probe must also fire for sticky assignments',
  );
});

// --- max_concurrency placement ---

// A producer block carries the same queue = "…flag-logs-queue" line but has
// no max_batch_timeout, so matching on the queue name alone leaks the cap
// into whichever consumer comes next — in practice the events consumer.
test('max_concurrency lands on every flag-logs consumer and no other', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const awkMatch = script.match(/awk -v mc="\$CONSUMER_CONCURRENCY_TOML" '([\s\S]*?)'\s*wrangler\.toml/);
  assert.ok(awkMatch, 'max_concurrency awk not found in script.sh');

  const directory = mkdtempSync(join(tmpdir(), 'mc-test-'));
  try {
    copyFileSync(wranglerTomlPath, join(directory, 'wrangler.toml'));
    // Append a second flag-log shard, consumer + producer, as the deployer does.
    const extra = [
      '', '[[queues.consumers]]', 'queue = "flag-logs-queue-2"',
      'max_batch_size = 100', 'max_batch_timeout = 10', '',
      '[[queues.producers]]', 'queue = "flag-logs-queue-2"',
      'binding = "flag_logs_queue_2"', '',
    ].join('\n');
    writeFileSync(join(directory, 'wrangler.toml'),
      readFileSync(join(directory, 'wrangler.toml'), 'utf8') + extra);

    const r = spawnSync('bash', ['-c',
      `awk -v mc="max_concurrency = 7" '${awkMatch[1]}' wrangler.toml > out.toml`,
    ], { cwd: directory, encoding: 'utf8', timeout: 5000 });
    assert.equal(r.status, 0, r.stderr);

    const out = readFileSync(join(directory, 'out.toml'), 'utf8');
    const blocks = out.match(/\[\[queues\.consumers\]\]\n(?:[^[]*)/g) || [];
    assert.ok(blocks.length >= 3, `expected 3 consumers, got ${blocks.length}`);
    for (const b of blocks) {
      const queue = /queue = "([^"]+)"/.exec(b)[1];
      const capped = b.includes('max_concurrency');
      const shouldBeCapped = queue.includes('flag-logs');
      assert.equal(capped, shouldBeCapped,
        `consumer ${queue}: capped=${capped}, expected ${shouldBeCapped}`);
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

// --- Secret hygiene ---

// Wrangler prints [vars] values in every deploy log, and Cloudflare stores
// them as plaintext readable by anyone with dashboard or API read access —
// so the client secret must only ever live in the Worker secret store.
test('client secret never lands in the generated wrangler.toml', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  assert.doesNotMatch(script, /CLIENT_SECRET_TOML/, 'no TOML-escaped client secret variable');
  assert.doesNotMatch(script, /print "CONFIDENCE_CLIENT_SECRET/, 'no [vars] injection in the awk step');
  const template = readFileSync(wranglerTomlPath, 'utf8');
  assert.doesNotMatch(template, /^CONFIDENCE_CLIENT_SECRET *=/m, 'template must not carry a var placeholder');
});

// The deploy log must never print any part of the Cloudflare API token:
// even a five-character prefix meaningfully narrows a brute force.
test('deploy log never prints any part of the Cloudflare API token', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  assert.doesNotMatch(script, /\$\{CLOUDFLARE_API_TOKEN:[0-9$]/, 'no token substring may be echoed');
});

// The vars the deployer does inject must keep flowing into [vars] — the
// table is now empty in the template — and must never include the secret.
test('vars injection still populates [vars] — and never with the client secret', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const awkMatch = script.match(/awk -v allowed="\$\{ALLOWED_ORIGIN_TOML\}[\s\S]*?&& mv wrangler\.toml\.new wrangler\.toml/);
  assert.ok(awkMatch, 'vars injection awk not found in script.sh');

  const directory = mkdtempSync(join(tmpdir(), 'vars-injection-test-'));
  try {
    copyFileSync(wranglerTomlPath, join(directory, 'wrangler.toml'));
    const r = spawnSync('bash', ['-c', `
set -euo pipefail
ALLOWED_ORIGIN_TOML="https://example.com"
ETAG_TOML="etag-123"
DEPLOYER_VERSION="v1.2.3"
DEPLOY_CONFIG_HASH="abc123"
FORCE_APPLY=""
ENABLE_APPLY_DEDUP=""
FLAG_LOG_SINK="queue"
${awkMatch[0]}
`], { cwd: directory, encoding: 'utf8', timeout: 5000 });
    assert.equal(r.status, 0, r.stderr);

    const config = readFileSync(join(directory, 'wrangler.toml'), 'utf8');
    const parsed = parseToml(config);
    assert.equal(parsed.vars.FLAG_LOG_SINK, 'queue', 'generated config must stay valid TOML');
    assert.match(config, /ALLOWED_ORIGIN = "https:\/\/example\.com"/);
    assert.match(config, /RESOLVER_STATE_ETAG = "etag-123"/);
    assert.match(config, /DEPLOYER_VERSION = "v1\.2\.3"/);
    assert.match(config, /DEPLOY_CONFIG_HASH = "abc123"/);
    assert.match(config, /FLAG_LOG_SINK = "queue"/);
    assert.doesNotMatch(config, /^CONFIDENCE_CLIENT_SECRET *=/m);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

// Runs the real store_worker_secret out of script.sh against a stubbed
// curl/jq pair, so the secret PUT the deployer makes (resolve-token key,
// skip-path repair) is asserted end-to-end without touching the network.
const STORE_WORKER_SECRET = (() => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const match = script.match(/^store_worker_secret\(\) \{[\s\S]*?^\}/m);
  assert.ok(match, 'could not find store_worker_secret in script.sh');
  return match[0];
})();

function runStoreWorkerSecret(status) {
  const directory = mkdtempSync(join(tmpdir(), 'worker-secret-test-'));
  try {
    writeFileSync(join(directory, 'jq'), `#!/bin/bash
name=""
text=""
while [ $# -gt 0 ]; do
  if [ "$1" = "--arg" ]; then
    if [ "$2" = "name" ]; then name="$3"; fi
    if [ "$2" = "text" ]; then text="$3"; fi
    shift 3
  else
    shift
  fi
done
printf '{"name":"%s","text":"%s","type":"secret_text"}' "$name" "$text"
`, { mode: 0o755 });

    writeFileSync(join(directory, 'curl'), `#!/bin/bash
method=""
url=""
body=""
while [ $# -gt 0 ]; do
  case "$1" in
    -d) body="$2"; shift 2 ;;
    -X) method="$2"; shift 2 ;;
    https://*) url="$1"; shift ;;
    *) shift ;;
  esac
done
printf '%s %s\\n%s\\n' "$method" "$url" "$body" >> "$STUB_DIR/requests"
echo -n "$STATUS"
`, { mode: 0o755 });

    const result = spawnSync('bash', ['-c', `
set -euo pipefail
export PATH="${directory}:$PATH"
export STUB_DIR="${directory}"
export STATUS="${status}"
CLOUDFLARE_API_TOKEN=tok
CLOUDFLARE_ACCOUNT_ID=acct123
WORKER_NAME=worker-test
${STORE_WORKER_SECRET}
store_worker_secret "CONFIDENCE_CLIENT_SECRET" "super-secret-value"
`], { encoding: 'utf8', timeout: 5000 });
    const requests = existsSync(join(directory, 'requests'))
      ? readFileSync(join(directory, 'requests'), 'utf8')
      : '';
    return { ...result, requests };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test('store_worker_secret PUTs a secret_text to the Worker secrets endpoint', () => {
  const r = runStoreWorkerSecret('200');
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.requests,
    /PUT https:\/\/api\.cloudflare\.com\/client\/v4\/accounts\/acct123\/workers\/scripts\/worker-test\/secrets/);
  assert.match(r.requests, /"name":"CONFIDENCE_CLIENT_SECRET"/);
  assert.match(r.requests, /"text":"super-secret-value"/);
  assert.match(r.requests, /"type":"secret_text"/);
  assert.match(r.stdout, /CONFIDENCE_CLIENT_SECRET stored as worker secret/);
});

test('store_worker_secret fails loudly on a non-2xx response', () => {
  const r = runStoreWorkerSecret('500');
  assert.equal(r.status, 1);
  assert.match(r.stderr, /Could not store CONFIDENCE_CLIENT_SECRET as worker secret \(HTTP 500\)/);
});

test('the secret value never appears in deployer output', () => {
  for (const status of ['200', '500']) {
    const r = runStoreWorkerSecret(status);
    assert.doesNotMatch(r.stdout + r.stderr, /super-secret-value/,
      `status ${status}: the secret value must not be printed`);
  }
});

// --- Deploy-time secret upload ---

// --secrets-file uploads secrets atomically with the version and never
// deletes omitted secrets (wrangler 4.x), so the live worker has the client
// secret from the version's first request — fresh install and var→secret
// migration alike, with no window where the binding is missing.
test('wrangler deploy carries the client secret via --secrets-file', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const deployLine = script.match(/wrangler deploy "\$\{WRANGLER_DEPLOY_ARGS_ARRAY\[@\]\}"[^\n]*/);
  assert.ok(deployLine, 'wrangler deploy line not found');
  assert.match(deployLine[0], /--secrets-file "\$SECRETS_FILE"/,
    'the deploy itself must upload the secrets file');
  assert.match(deployLine[0], /--no-bundle/);

  const buildAt = script.indexOf('SECRETS_FILE=$(mktemp)');
  const jqAt = script.indexOf('jq -n --arg v "$CONFIDENCE_CLIENT_SECRET" \'{"CONFIDENCE_CLIENT_SECRET": $v}\' > "$SECRETS_FILE"');
  const deployAt = script.indexOf(deployLine[0]);
  const cleanupMatch = script.match(/^ {5}rm -f "\$SECRETS_FILE"$/m);
  assert.ok(cleanupMatch, 'explicit secrets file cleanup not found');
  const cleanupAt = script.indexOf(cleanupMatch[0]);
  assert.ok(buildAt !== -1 && jqAt !== -1 && cleanupAt !== -1, 'secrets file plumbing not found');
  assert.ok(buildAt < jqAt && jqAt < deployAt && deployAt < cleanupAt,
    'secrets file must be created and filled before the deploy, removed after');
});

// --- Skip-path repair ---

// The ETag-skip path exits without deploying, so a worker missing its client
// secret (e.g. one still carrying the pre-fix plaintext var) would stay
// broken until the next state change — the branch must verify and repair.
test('the ETag-skip path repairs a worker missing the client secret', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const branch = script.match(/if \[ "\$HTTP_STATUS" = "304" \][\s\S]*?elif \[ "\$HTTP_STATUS" = "200" \]/);
  assert.ok(branch, '304 skip branch not found');
  assert.match(branch[0], /ensure_worker_secret "CONFIDENCE_CLIENT_SECRET" "\$CONFIDENCE_CLIENT_SECRET"/,
    'skip path must repair the client secret');
  assert.match(branch[0], /NO_DEPLOY/, 'repair must respect NO_DEPLOY');
  assert.match(branch[0], /exit 1/, 'a confirmed-missing secret with a failed repair must fail the run');
  assert.match(branch[0], /REPAIR_STATUS/, 'wiring must distinguish repair outcomes');
  assert.match(branch[0], /next run will retry/, 'an inconclusive check must only warn, not fail the run');
});

// Runs the real ensure_worker_secret (with the real store_worker_secret)
// out of script.sh against a stubbed curl/jq pair. The jq stub answers the
// -r binding classification with the baked value (secret_text / plain_text /
// absent); GET_STATUS and PUT_STATUS are what the stubbed settings GET and
// secret PUT return respectively.
const WORKER_BINDING_TYPE = (() => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const match = script.match(/^worker_binding_type\(\) \{[\s\S]*?^\}/m);
  assert.ok(match, 'could not find worker_binding_type in script.sh');
  return match[0];
})();

const ENSURE_WORKER_SECRET = (() => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const match = script.match(/^ensure_worker_secret\(\) \{[\s\S]*?^\}/m);
  assert.ok(match, 'could not find ensure_worker_secret in script.sh');
  return match[0];
})();

function runEnsureWorkerSecret(getStatus, putStatus, jqOut) {
  const directory = mkdtempSync(join(tmpdir(), 'ensure-secret-test-'));
  try {
    writeFileSync(join(directory, 'jq'), `#!/bin/bash
if [ "$1" = "-r" ]; then echo -n "${jqOut}"; exit 0; fi
name=""
text=""
while [ $# -gt 0 ]; do
  if [ "$1" = "--arg" ]; then
    if [ "$2" = "name" ]; then name="$3"; fi
    if [ "$2" = "text" ]; then text="$3"; fi
    shift 3
  else
    shift
  fi
done
printf '{"name":"%s","text":"%s","type":"secret_text"}' "$name" "$text"
`, { mode: 0o755 });

    writeFileSync(join(directory, 'curl'), `#!/bin/bash
method=""
url=""
body=""
while [ $# -gt 0 ]; do
  case "$1" in
    -d) body="$2"; shift 2 ;;
    -X) method="$2"; shift 2 ;;
    https://*) url="$1"; shift ;;
    *) shift ;;
  esac
done
printf '%s %s\\n%s\\n' "$method" "$url" "$body" >> "$STUB_DIR/requests"
if [ -n "$method" ]; then echo -n "$PUT_STATUS"; else echo -n "$GET_STATUS"; fi
`, { mode: 0o755 });

    const result = spawnSync('bash', ['-c', `
set -euo pipefail
export PATH="${directory}:$PATH"
export STUB_DIR="${directory}"
export GET_STATUS="${getStatus}"
export PUT_STATUS="${putStatus}"
CLOUDFLARE_API_TOKEN=tok
CLOUDFLARE_ACCOUNT_ID=acct123
WORKER_NAME=worker-test
${STORE_WORKER_SECRET}

${WORKER_BINDING_TYPE}

${ENSURE_WORKER_SECRET}
ensure_worker_secret "CONFIDENCE_CLIENT_SECRET" "super-secret-value"
`], { encoding: 'utf8', timeout: 5000 });
    const requests = existsSync(join(directory, 'requests'))
      ? readFileSync(join(directory, 'requests'), 'utf8')
      : '';
    return { ...result, requests };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test('ensure_worker_secret: already present as a secret → no PUT, quiet success', () => {
  const r = runEnsureWorkerSecret('200', '500', 'secret_text');
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /already present as worker secret/);
  assert.doesNotMatch(r.requests, /PUT/, 'must not PUT when the secret exists');
  assert.match(r.requests, /https:\/\/api\.cloudflare\.com\/client\/v4\/accounts\/acct123\/workers\/scripts\/worker-test\/settings/);
});

test('ensure_worker_secret: missing (no binding at all) → repair PUT', () => {
  const r = runEnsureWorkerSecret('200', '200', 'absent');
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.requests, /PUT https:\/\/api\.cloudflare\.com\/client\/v4\/accounts\/acct123\/workers\/scripts\/worker-test\/secrets/);
  assert.match(r.requests, /"name":"CONFIDENCE_CLIENT_SECRET"/);
  assert.match(r.stdout, /stored as worker secret/);
});

test('ensure_worker_secret: still a plaintext var → no doomed PUT, precise message', () => {
  const r = runEnsureWorkerSecret('200', '200', 'plain_text');
  assert.equal(r.status, 1);
  assert.doesNotMatch(r.requests, /PUT/, 'a secret PUT over an existing var binding is rejected by Cloudflare — it must not be attempted');
  assert.match(r.stderr, /still a plaintext var/);
  assert.match(r.stderr, /FORCE_DEPLOY=1/);
});

test('ensure_worker_secret: confirmed missing + failed PUT → exit 1', () => {
  const r = runEnsureWorkerSecret('200', '500', 'absent');
  assert.equal(r.status, 1);
  assert.match(r.stderr, /Could not store CONFIDENCE_CLIENT_SECRET as worker secret \(HTTP 500\)/);
});

test('ensure_worker_secret: unreachable settings → inconclusive exit 2, no PUT', () => {
  const r = runEnsureWorkerSecret('503', '200', 'absent');
  assert.equal(r.status, 2);
  assert.doesNotMatch(r.requests, /PUT/, 'must not PUT when presence is unknown');
  assert.match(r.stderr, /Could not read worker settings \(HTTP 503\)/);
});

// --- Deployer version bootstrap ---

// Docker deploys never had a version: the image does not bake .release_tag
// and contains no git checkout, so the version-based redeploy never fired.
// These tests pin the two halves of the fix: the version falls back to the
// image's COMMIT_SHA env, and a worker reporting no version is force
// redeployed once (which is what migrates old workers).
const DEPLOYER_VERSION_BLOCK = (() => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const match = script.match(/DEPLOYER_VERSION=""\nif command -v git[\s\S]*?^fi$/m);
  assert.ok(match, 'could not find the deployer version block');
  return match[0];
})();

const VERSION_FORCE_BLOCK = (() => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const match = script.match(/# If the deployer version changed[\s\S]*?^fi$/m);
  assert.ok(match, 'could not find the version force block');
  return match[0];
})();

function runBash(scriptBody, env = {}) {
  return spawnSync('bash', ['-c', scriptBody], {
    encoding: 'utf8', timeout: 5000,
    env: { ...process.env, ...env },
  });
}

test('deployer version falls back to the image COMMIT_SHA when no tag is baked', () => {
  const directory = mkdtempSync(join(tmpdir(), 'deployer-version-test-'));
  try {
    const withSha = runBash(`
set -euo pipefail
cd "${directory}"
COMMIT_SHA=ce1e2901deadbeef
${DEPLOYER_VERSION_BLOCK}
echo "resolved:\${DEPLOYER_VERSION:-}"
`, { COMMIT_SHA: '' });
    assert.equal(withSha.status, 0, withSha.stderr);
    assert.match(withSha.stdout, /Deployer version \(image commit\): ce1e2901deadbeef/);
    assert.match(withSha.stdout, /resolved:ce1e2901deadbeef/);

    const withoutSha = runBash(`
set -euo pipefail
cd "${directory}"
${DEPLOYER_VERSION_BLOCK}
echo "resolved:\${DEPLOYER_VERSION:-}"
`, { COMMIT_SHA: '' });
    assert.equal(withoutSha.status, 0, withoutSha.stderr);
    assert.match(withoutSha.stdout, /resolved:$/m);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('version force fires on change and on versionless workers, not when matched', () => {
  const run = (prev, cur) => runBash(`
set -euo pipefail
PREV_DEPLOYER_VERSION="${prev}"
DEPLOYER_VERSION="${cur}"
FORCE_DEPLOY=""
${VERSION_FORCE_BLOCK}
[ -n "$FORCE_DEPLOY" ] && echo FORCED || echo NOT_FORCED
`);
  assert.match(run('', 'abc123').stdout, /FORCED/,
    'a versionless worker with a versioned deployer must force one migration deploy');
  assert.match(run('old', 'new').stdout, /FORCED/, 'a version change must force');
  assert.match(run('same', 'same').stdout, /NOT_FORCED/, 'matching versions must not force');
  assert.match(run('old', '').stdout, /NOT_FORCED/, 'a versionless deployer must not force');
});

// The secrets file carries the cleartext client secret; it must be removed
// on every exit path — a failed jq/deploy, an abort, a signal — not only the
// happy one.
test('the secrets file cleanup trap is armed before first use', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const mktempAt = script.indexOf('SECRETS_FILE=$(mktemp)');
  const trapAt = script.indexOf('trap \'rm -f "$SECRETS_FILE"\' EXIT');
  const signalTrapAt = script.indexOf("trap 'exit 143' TERM");
  const jqAt = script.indexOf('jq -n --arg v "$CONFIDENCE_CLIENT_SECRET"');
  const cleanupMatch = script.match(/^ {5}rm -f "\$SECRETS_FILE"$/m);
  assert.ok(cleanupMatch, 'explicit secrets file cleanup not found');
  const rmAt = script.indexOf(cleanupMatch[0]);
  const clearAt = script.indexOf('trap - EXIT INT TERM');
  assert.ok(mktempAt !== -1 && trapAt !== -1 && signalTrapAt !== -1 && jqAt !== -1 && clearAt !== -1,
    'secrets file plumbing not found');
  assert.ok(mktempAt < trapAt && trapAt < jqAt,
    'the EXIT trap must be armed between mktemp and the first write to the file');
  assert.ok(signalTrapAt > trapAt, 'INT/TERM handlers must route through the EXIT trap');
  assert.ok(rmAt > jqAt && clearAt > rmAt,
    'explicit cleanup after the deploy, traps cleared after');
});

// A hung Cloudflare API call must never hang the deployer: the two API curls
// this deployer adds carry the same timeouts the resolve-key lookup got
// after the silent-key-regeneration incident.
test('the repair curls carry connect/max timeouts', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const storeFn = script.match(/^store_worker_secret\(\) \{[\s\S]*?^\}/m);
  const lookupFn = script.match(/^worker_binding_type\(\) \{[\s\S]*?^\}/m);
  assert.ok(storeFn, 'store_worker_secret not found');
  assert.ok(lookupFn, 'worker_binding_type not found');
  assert.match(storeFn[0], /--connect-timeout 10 --max-time 30/,
    'the secret PUT must have timeouts');
  assert.match(lookupFn[0], /--connect-timeout 10 --max-time 30/,
    'the settings GET must have timeouts');
});

// --- Resolve-token key lookup ---

// Runs the real key block (with the real worker_binding_type and real jq)
// against a stubbed curl. A new key must only be generated when the worker is
// confirmed to have none: generating one otherwise replaces the existing key
// and invalidates every outstanding resolve token.
const RESOLVE_KEY_BLOCK = (() => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const match = script.match(/^SET_SECRET_AFTER_DEPLOY=""\n[\s\S]*?^fi$/m);
  assert.ok(match, 'could not find the resolve-token key block');
  return match[0];
})();

function runResolveKeyLookup(status, body, { providedKey = '', curlFails = false } = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'resolve-key-test-'));
  try {
    writeFileSync(join(directory, 'body'), body);
    writeFileSync(join(directory, 'curl'), curlFails
      ? '#!/bin/bash\nexit 28\n'
      : `#!/bin/bash\ncat "$STUB_DIR/body"; echo -n "${status}"\n`, { mode: 0o755 });
    const result = spawnSync('bash', ['-c', `
set -euo pipefail
export PATH="${directory}:$PATH"
export STUB_DIR="${directory}"
CLOUDFLARE_API_TOKEN=tok
CLOUDFLARE_ACCOUNT_ID=acct123
WORKER_NAME=worker-test
RESOLVE_TOKEN_ENCRYPTION_KEY="${providedKey}"
${WORKER_BINDING_TYPE}
${RESOLVE_KEY_BLOCK}
echo "SET_SECRET=$SET_SECRET_AFTER_DEPLOY KEY_LEN=\${#RESOLVE_TOKEN_ENCRYPTION_KEY}"
`], { encoding: 'utf8', timeout: 5000 });
    return result;
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

const settingsWith = (bindings) => JSON.stringify({ success: true, result: { bindings } });

test('resolve key: 200 with the key bound → keep it, no new key', () => {
  const r = runResolveKeyLookup('200', settingsWith([{ name: 'RESOLVE_TOKEN_ENCRYPTION_KEY', type: 'secret_text' }]));
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /Existing RESOLVE_TOKEN_ENCRYPTION_KEY found/);
  assert.match(r.stdout, /SET_SECRET= KEY_LEN=0/);
});

test('resolve key: 200 without the key → generate one', () => {
  const r = runResolveKeyLookup('200', settingsWith([{ name: 'OTHER', type: 'plain_text' }]));
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /Auto-generating/);
  assert.match(r.stdout, /SET_SECRET=true KEY_LEN=24/);
});

test('resolve key: 404 script-not-found (10007) → first deploy, generate one', () => {
  const r = runResolveKeyLookup('404', JSON.stringify({ success: false, errors: [{ code: 10007, message: 'workers.api.error.script_not_found' }] }));
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /SET_SECRET=true KEY_LEN=24/);
});

test('resolve key: 404 legacy-environment-not-found (10090) → first deploy, generate one', () => {
  const r = runResolveKeyLookup('404', JSON.stringify({ success: false, errors: [{ code: 10090, message: 'environment not found' }] }));
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /SET_SECRET=true KEY_LEN=24/);
});

test('resolve key: 404 with no error codes → refuse, exit 1', () => {
  const r = runResolveKeyLookup('404', '{}');
  assert.equal(r.status, 1);
  assert.match(r.stderr, /not a missing-worker error/);
  assert.doesNotMatch(r.stdout, /Auto-generating/);
});

test('resolve key: any other 404 → refuse, exit 1', () => {
  const r = runResolveKeyLookup('404', JSON.stringify({ success: false, errors: [{ code: 7003, message: 'Could not route' }] }));
  assert.equal(r.status, 1);
  assert.match(r.stderr, /refusing to generate a new one/);
  assert.doesNotMatch(r.stdout, /Auto-generating/);
});

for (const status of ['500', '429', '403']) {
  test(`resolve key: HTTP ${status} → refuse, exit 1`, () => {
    const r = runResolveKeyLookup(status, '{}');
    assert.equal(r.status, 1);
    assert.match(r.stderr, new RegExp(`HTTP ${status}`));
    assert.doesNotMatch(r.stdout, /Auto-generating/);
  });
}

test('resolve key: curl timeout (000) → refuse, exit 1', () => {
  const r = runResolveKeyLookup('', '', { curlFails: true });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /HTTP 000/);
  assert.doesNotMatch(r.stdout, /Auto-generating/);
});

test('resolve key: unparseable 200 body → refuse, exit 1', () => {
  const r = runResolveKeyLookup('200', '<html>oops</html>');
  assert.equal(r.status, 1);
  assert.match(r.stderr, /Could not parse/);
});

test('resolve key: provided explicitly → no lookup at all', () => {
  const r = runResolveKeyLookup('500', '{}', { providedKey: 'AAAAAAAAAAAAAAAAAAAAAA==' });
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /RESOLVE_TOKEN_ENCRYPTION_KEY provided/);
  assert.doesNotMatch(r.stdout, /Checking for existing/);
});

// --- Deploy config fingerprint ---

// The fingerprint is what makes a rotated secret or changed setting redeploy
// when the resolver state is unchanged; without it, skipping on 304 would
// leave the worker on stale config until the next flag change.
const DEPLOY_CONFIG_HASH_FN = (() => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const match = script.match(/^deploy_config_hash\(\) \{[\s\S]*?^\}/m);
  assert.ok(match, 'could not find deploy_config_hash in script.sh');
  return match[0];
})();

const CONFIG_FORCE_BLOCK = (() => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const match = script.match(/# If the deploy config changed[\s\S]*?^fi$/m);
  assert.ok(match, 'could not find the deploy config force block');
  return match[0];
})();

function configHash(overrides = {}) {
  const vars = {
    CONFIDENCE_CLIENT_SECRET: 'secret-1', INITIAL_WORKDIR: '/tmp',
    RESOLVE_TOKEN_ENCRYPTION_KEY: '', CONFIDENCE_RESOLVER_ALLOWED_ORIGIN: '',
    FORCE_APPLY: '', ENABLE_APPLY_DEDUP: '', ENABLE_STICKY_ASSIGNMENTS: '',
    WRANGLER_DEPLOY_ARGS: '', WRANGLER_CONFIG_APPEND_FILE: '', WRANGLER_DEPLOY_ARGS_FILE: '',
    ...overrides,
  };
  const assigns = Object.entries(vars).map(([k, v]) => `${k}='${v}'`).join('\n');
  const r = runBash(`set -euo pipefail\n${assigns}\n${DEPLOY_CONFIG_HASH_FN}\ndeploy_config_hash`);
  assert.equal(r.status, 0, r.stderr);
  return r.stdout.trim();
}

test('deploy config hash is a stable hex HMAC that never contains the secret', () => {
  const h = configHash();
  assert.match(h, /^[0-9a-f]{64}$/);
  assert.equal(configHash(), h, 'same config must hash the same');
  assert.ok(!h.includes('secret-1'));
});

test('deploy config hash changes on secret rotation and setting changes', () => {
  const base = configHash();
  assert.notEqual(configHash({ CONFIDENCE_CLIENT_SECRET: 'secret-2' }), base, 'client secret rotation');
  assert.notEqual(configHash({ RESOLVE_TOKEN_ENCRYPTION_KEY: 'k2' }), base, 'resolve key rotation');
  assert.notEqual(configHash({ FLAG_LOG_SINK: 'buffer' }), base, 'FLAG_LOG_SINK');
  assert.notEqual(configHash({ CONFIDENCE_RESOLVER_ALLOWED_ORIGIN: 'https://x' }), base, 'ALLOWED_ORIGIN');
  assert.notEqual(configHash({ FLAG_LOGS_QUEUE_COUNT: '2' }), base, 'queue count');
});

test('deploy config hash covers the append file contents', () => {
  const directory = mkdtempSync(join(tmpdir(), 'config-hash-test-'));
  try {
    writeFileSync(join(directory, 'extra.toml'), 'a = 1\n');
    const one = configHash({ INITIAL_WORKDIR: directory, WRANGLER_CONFIG_APPEND_FILE: 'extra.toml' });
    writeFileSync(join(directory, 'extra.toml'), 'a = 2\n');
    const two = configHash({ INITIAL_WORKDIR: directory, WRANGLER_CONFIG_APPEND_FILE: 'extra.toml' });
    assert.notEqual(one, two);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('config force fires on change and on fingerprintless workers, not on match or blip', () => {
  const run = (stored, { prevEtag = 'e1', force = '' } = {}) => runBash(`
set -euo pipefail
worker_binding_type() { echo "${stored}"; }
PREV_ETAG="${prevEtag}"
DEPLOY_CONFIG_HASH="h1"
FORCE_DEPLOY="${force}"
${CONFIG_FORCE_BLOCK}
[ -n "$FORCE_DEPLOY" ] && echo FORCED || echo NOT_FORCED
`);
  assert.match(run('h1').stdout, /NOT_FORCED/, 'matching fingerprint must not force');
  assert.match(run('h0').stdout, /FORCED/, 'changed fingerprint must force');
  assert.match(run('absent').stdout, /FORCED/, 'a worker without a fingerprint must force once');
  assert.match(run('unknown').stdout, /NOT_FORCED/, 'an API blip must not force');
  assert.match(run('h0', { prevEtag: '' }).stdout, /NOT_FORCED/, 'no previous etag: nothing to skip, no lookup needed');
});
