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
