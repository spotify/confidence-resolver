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
