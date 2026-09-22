const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const { mkdtempSync, readFileSync, copyFileSync, rmSync, writeFileSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { test } = require('node:test');

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

function runSinkValidation(value) {
  return spawnSync('bash', ['-c', `
set -euo pipefail
FLAG_LOG_SINK=${value === undefined ? '${FLAG_LOG_SINK:-queue}' : `"${value}"`}
FLAG_LOG_SINK=$(printf '%s' "$FLAG_LOG_SINK" | tr '[:upper:]' '[:lower:]')
if [ "$FLAG_LOG_SINK" != "queue" ] && [ "$FLAG_LOG_SINK" != "logpush" ]; then
    echo "FLAG_LOG_SINK must be \\"queue\\" or \\"logpush\\", got: $FLAG_LOG_SINK" >&2
    exit 1
fi
echo "$FLAG_LOG_SINK"
`], { encoding: 'utf8', timeout: 5000, env: { ...process.env, FLAG_LOG_SINK: '' } });
}

for (const value of ['logpush', 'LOGPUSH', 'LogPush', 'queue', 'QUEUE']) {
  test(`accepts sink ${value} and lowercases it`, () => {
    const result = runSinkValidation(value);
    assert.equal(result.status, 0);
    assert.equal(result.stdout.trim(), value.toLowerCase());
  });
}

for (const value of ['', 'r2', 'logpsuh', 'true', 'queues']) {
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

test('logpush mode refuses to deploy without R2 credentials', () => {
  const result = spawnSync('bash', ['-c', `
set -uo pipefail
FLAG_LOG_SINK=logpush
if [ -z "\${R2_ACCESS_KEY_ID:-}" ] || [ -z "\${R2_SECRET_ACCESS_KEY:-}" ]; then
    echo "FLAG_LOG_SINK=logpush requires R2_ACCESS_KEY_ID and R2_SECRET_ACCESS_KEY" >&2
    exit 1
fi
echo "OK"
`], { encoding: 'utf8', timeout: 5000, env: { ...process.env, R2_ACCESS_KEY_ID: '', R2_SECRET_ACCESS_KEY: '' } });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /requires R2_ACCESS_KEY_ID/);
});

// The Logpush job must be scoped to this worker and exclude cron invocations:
// without the ScriptName filter it captures every Worker in the account, and
// without the EventType filter the aggregator's own console output feeds back
// into the bucket it is draining.
test('logpush job filter is a JSON string scoped to the worker and fetch events', () => {
  const result = spawnSync('bash', ['-c', `
jq -n --arg script "my-worker" '{
    filter: ({where: {and: [
        {key: "ScriptName", operator: "eq", value: $script},
        {key: "EventType", operator: "eq", value: "fetch"}
    ]}} | tostring)
}'
`], { encoding: 'utf8', timeout: 5000 });
  assert.equal(result.status, 0);
  const body = JSON.parse(result.stdout);
  assert.equal(typeof body.filter, 'string', 'filter must be a JSON-encoded string');
  assert.deepEqual(JSON.parse(body.filter), {
    where: {
      and: [
        { key: 'ScriptName', operator: 'eq', value: 'my-worker' },
        { key: 'EventType', operator: 'eq', value: 'fetch' },
      ],
    },
  });
});

// logpush = true is a top-level key, so appending it would land it inside
// whichever table happens to be last. It has to be prepended.
test('logpush = true is prepended above every table', () => {
  const directory = mkdtempSync(join(tmpdir(), 'flag-log-logpush-test-'));
  try {
    copyFileSync(wranglerTomlPath, join(directory, 'wrangler.toml'));
    const result = spawnSync('bash', ['-c', `
set -euo pipefail
sed -i.tmp '/^logpush *= *.*$/d' wrangler.toml || true
LOGPUSH_TMPFILE=./wrangler.toml.logpush
printf 'logpush = true\\n' > "$LOGPUSH_TMPFILE"
cat wrangler.toml >> "$LOGPUSH_TMPFILE"
mv "$LOGPUSH_TMPFILE" wrangler.toml
cat >> wrangler.toml <<EOF

[[r2_buckets]]
binding = "FLAG_LOGS_R2"
bucket_name = "flag-logs"

[triggers]
crons = ["* * * * *"]
EOF
`], { cwd: directory, encoding: 'utf8', timeout: 5000, env: { ...process.env, TMPDIR: directory } });
    assert.equal(result.status, 0, result.stderr);
    const config = readFileSync(join(directory, 'wrangler.toml'), 'utf8');
    const logpushLine = config.split('\n').findIndex(l => l.trim() === 'logpush = true');
    const firstTable = config.split('\n').findIndex(l => l.trim().startsWith('['));
    assert.ok(logpushLine >= 0, 'logpush = true must be present');
    assert.ok(logpushLine < firstTable, 'logpush = true must precede the first table');
    assert.match(config, /binding = "FLAG_LOGS_R2"/);
    assert.match(config, /crons = \["\* \* \* \* \*"\]/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

// Running the deployer twice must not accumulate duplicate keys.
test('re-running logpush setup does not duplicate logpush = true', () => {
  const directory = mkdtempSync(join(tmpdir(), 'flag-log-logpush-idem-'));
  try {
    copyFileSync(wranglerTomlPath, join(directory, 'wrangler.toml'));
    const script = `
set -euo pipefail
sed -i.tmp '/^logpush *= *.*$/d' wrangler.toml || true
LOGPUSH_TMPFILE=./wrangler.toml.logpush
printf 'logpush = true\\n' > "$LOGPUSH_TMPFILE"
cat wrangler.toml >> "$LOGPUSH_TMPFILE"
mv "$LOGPUSH_TMPFILE" wrangler.toml
`;
    const opts = { cwd: directory, encoding: 'utf8', timeout: 5000, env: { ...process.env, TMPDIR: directory } };
    assert.equal(spawnSync('bash', ['-c', script], opts).status, 0);
    assert.equal(spawnSync('bash', ['-c', script], opts).status, 0);
    const config = readFileSync(join(directory, 'wrangler.toml'), 'utf8');
    const occurrences = config.split('\n').filter(l => l.trim() === 'logpush = true').length;
    assert.equal(occurrences, 1);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
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
if [ "$FLAG_LOG_SINK" = "logpush" ]; then
    check_perm "R2 Storage" "Account > Workers R2 Storage > Edit" "$base/r2/buckets"
    check_perm "Logpush"    "Account > Logs > Edit"               "$base/logpush/jobs"
fi
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
  const r = runPreflight('logpush', {});
  assert.equal(r.status, 0, r.stderr);
  assert.equal((r.stdout.match(/PROBE_OK/g) || []).length, 4);
});

test('queue mode does not probe R2 or Logpush', () => {
  const r = runPreflight('queue', {});
  assert.equal(r.status, 0);
  assert.equal((r.stdout.match(/PROBE_OK/g) || []).length, 2);
  assert.doesNotMatch(r.stdout, /R2 Storage|Logpush/);
});

test('logpush mode probes R2 and Logpush', () => {
  const r = runPreflight('logpush', {});
  assert.match(r.stdout, /PROBE_OK R2 Storage/);
  assert.match(r.stdout, /PROBE_OK Logpush/);
});

test('a 403 on Logpush fails the deploy and names the scope', () => {
  const r = runPreflight('logpush', { 'logpush/jobs': 403 });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /PROBE_FAIL Logpush needs: Account > Logs > Edit/);
});

test('a 403 on R2 fails the deploy and names the scope', () => {
  const r = runPreflight('logpush', { 'r2/buckets': 403 });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /PROBE_FAIL R2 Storage needs: Account > Workers R2 Storage > Edit/);
});

// A token valid for one account returns an indistinguishable auth error for
// another, so a wrong-account token must fail here rather than part-way in.
test('a token with no access to the account fails every probe', () => {
  const r = runPreflight('logpush', {
    'workers/scripts': 403, queues: 403, 'r2/buckets': 403, 'logpush/jobs': 403,
  });
  assert.equal(r.status, 1);
  assert.equal((r.stderr.match(/PROBE_FAIL/g) || []).length, 4);
});

test('KV is only probed when metrics or sticky assignments are enabled', () => {
  assert.doesNotMatch(runPreflight('queue', {}).stdout, /Workers KV/);
  assert.match(runPreflight('queue', {}, { metrics: '1' }).stdout, /PROBE_OK Workers KV/);
});

// --- Rollback safety tests ---

// The aggregator trigger must exist under BOTH sinks. Under queue it is a
// no-op unless a bucket is bound, but without it a rollback from logpush
// leaves no drainer and Logpush keeps filling R2.




// The disable call must target the job by name and set enabled:false, or a
// rollback leaves Logpush writing into an undrained bucket.
test('logpush job disable sends enabled:false for the matching job only', () => {
  const r = spawnSync('bash', ['-c', `
echo '{"result":[{"id":"abc123","name":"loadtest-confidence-cloudflare-resolver-flag-logs"},{"id":"other","name":"unrelated-job"}]}' \
  | jq -r '.result[]? | select(.name == "loadtest-confidence-cloudflare-resolver-flag-logs") | .id'
echo '{"enabled": false}' | jq -c .
`], { encoding: 'utf8', timeout: 5000 });
  assert.equal(r.status, 0);
  const [id, body] = r.stdout.trim().split('\n');
  assert.equal(id, 'abc123');
  assert.deepEqual(JSON.parse(body), { enabled: false });
});

// --- Bucket-existence probe must distinguish "no bucket" from "no access" ---

function runBucketProbe(code) {
  const directory = mkdtempSync(join(tmpdir(), 'bucket-probe-'));
  try {
    writeFileSync(join(directory, 'curl'), `#!/bin/bash\necho -n "${code}"\n`, { mode: 0o755 });
    return spawnSync('bash', ['-c', `
set -uo pipefail
export PATH="${directory}:$PATH"
CLOUDFLARE_API_TOKEN=tok
CLOUDFLARE_ACCOUNT_ID=acct
r2_bucket_exists() {
    local CODE
    CODE=$(curl -sS -o /dev/null -w "%{http_code}" \
        -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
        "https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/r2/buckets/$1")
    case "$CODE" in
        200) return 0 ;;
        404) return 1 ;;
        *) echo "WARN could not determine bucket state (HTTP $CODE)" >&2; return 1 ;;
    esac
}
if r2_bucket_exists my-bucket; then echo EXISTS; else echo ABSENT; fi
`], { encoding: 'utf8', timeout: 5000 });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test('a 200 means the bucket exists and should be bound', () => {
  const r = runBucketProbe(200);
  assert.match(r.stdout, /EXISTS/);
  assert.doesNotMatch(r.stderr, /WARN/);
});

test('a 404 means no bucket, and that is not a problem worth warning about', () => {
  const r = runBucketProbe(404);
  assert.match(r.stdout, /ABSENT/);
  assert.doesNotMatch(r.stderr, /WARN/);
});

// A 403 is the common case on a queue-mode rollback token, which does not
// require R2 permissions. Silently reading it as "no bucket" would skip the
// drain and strand whatever Logpush already wrote.
test('a 403 is reported rather than silently treated as absent', () => {
  const r = runBucketProbe(403);
  assert.match(r.stdout, /ABSENT/);
  assert.match(r.stderr, /WARN could not determine bucket state \(HTTP 403\)/);
});

// --- Rollback ordering ---

// Disabling the Logpush job before the build would leave the previous
// logpush-mode worker emitting console lines with nothing capturing them for
// the length of a release build.
test('the Logpush job is disabled only after a successful deploy', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const setsFlag = script.indexOf('DISABLE_LOGPUSH_JOB_AFTER_DEPLOY=1');
  const deploys = script.indexOf('wrangler deploy "${WRANGLER_DEPLOY_ARGS_ARRAY[@]}"');
  const disables = script.indexOf('disable_logpush_job "${WORKER_NAME}-flag-logs"');
  assert.ok(setsFlag > 0 && deploys > 0 && disables > 0);
  assert.ok(setsFlag < deploys, 'the flag is set during sink provisioning');
  assert.ok(disables > deploys, 'the disable call must come after wrangler deploy');
});

// The default bucket name must not be something a customer plausibly already
// owns: the aggregator deletes the objects it claims.
test('the default bucket name is namespaced to Confidence', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  assert.match(script, /FLAG_LOGS_BUCKET="confidence-flag-logs"/);
  assert.doesNotMatch(script, /FLAG_LOGS_BUCKET="flag-logs"/);
});

// Objects must stay small enough that one fits the aggregator's per-object
// memory budget after the decode expansion.
test('logpush objects are pinned small', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const bytes = Number(/max_upload_bytes: (\d+)/.exec(script)[1]);
  assert.ok(bytes <= 4000000, `max_upload_bytes should be small, was ${bytes}`);
});

// --- Object-notification wiring ---

// R2 notifies on deletes too, and the consumer deletes everything it drains.
// An unscoped rule would therefore re-enqueue every key it just removed, and
// would also claim objects the customer put in the bucket themselves.
test('R2 notification rules are scoped to the prefixes this pipeline writes', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const fn = script.slice(script.indexOf('ensure_r2_notification()'),
                          script.indexOf('ensure_logpush_job()'));
  assert.match(fn, /"flag-logs\/" "overflow\/"/);
  assert.match(fn, /PutObject/);
  assert.match(fn, /CompleteMultipartUpload/);
  assert.doesNotMatch(fn, /DeleteObject/);
});

test('the object queue is created with a bounded consumer concurrency', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  assert.match(script, /ensure_queue "\$FLAG_LOGS_OBJECTS_QUEUE"/);
  assert.match(script, /max_concurrency = \$\{FLAG_LOGS_CONSUMER_CONCURRENCY\}/);
  assert.match(script, /max_retries = 5/);
});

// The cron aggregator is gone; R2 notifies per object instead.
test('no cron trigger is configured any more', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  assert.doesNotMatch(script, /\[triggers\]/);
  assert.doesNotMatch(script, /crons =/);
});

function runConcurrencyValidation(value) {
  return spawnSync('bash', ['-c', `
set -uo pipefail
FLAG_LOGS_CONSUMER_CONCURRENCY=${value}
if [[ ! "$FLAG_LOGS_CONSUMER_CONCURRENCY" =~ ^[1-9][0-9]*$ ]] \
    || [ "$FLAG_LOGS_CONSUMER_CONCURRENCY" -gt 250 ]; then
    echo "must be between 1 and 250" >&2
    exit 1
fi
echo OK
`], { encoding: 'utf8', timeout: 5000 });
}

for (const value of ['1', '10', '43', '250']) {
  test(`accepts consumer concurrency ${value}`, () => {
    assert.equal(runConcurrencyValidation(value).status, 0);
  });
}

// 250 is Cloudflare's per-queue cap; beyond it the deploy would silently get
// less parallelism than asked for, which is the wrong way to find out.
for (const value of ['0', '-1', '251', 'abc', '1.5']) {
  test(`rejects consumer concurrency ${value}`, () => {
    const r = runConcurrencyValidation(value);
    assert.equal(r.status, 1);
    assert.match(r.stderr, /between 1 and 250/);
  });
}

// Sized against the measured 4 MiB backend limit: above it the delivery is
// rejected with 413 and no retry recovers.
test('logpush object size is sized under the measured backend limit', () => {
  const script = readFileSync(join(__dirname, 'script.sh'), 'utf8');
  const records = Number(/max_upload_records: (\d+)/.exec(script)[1]);
  assert.ok(records <= 1000, `max_upload_records should leave headroom, was ${records}`);
});
