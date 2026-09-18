const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const { mkdtempSync, readFileSync, copyFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { test } = require('node:test');

const scriptPath = join(__dirname, 'script.sh');
const script = readFileSync(scriptPath, 'utf8');
function section(start, end) {
  const from = script.indexOf(start);
  const to = script.indexOf(end, from);
  assert(from >= 0 && to > from, `Missing script section: ${start}`);
  return script.slice(from, to);
}

const validation = section('FLAG_LOG_QUEUE_COUNT=', '# CDN base URL');
const provisioning = section('# Determine queue name based on prefix', '# Create events queue');
const configuration = section('# Update worker name and queue names', '# Prepare ALLOWED_ORIGIN');

function run({ count, prefix = '', existing = false, createStatus = '201' } = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'flag-log-queues-test-'));
  try {
    copyFileSync(join(__dirname, '../wrangler.toml'), join(directory, 'wrangler.toml'));
    const namePrefix = prefix ? `${prefix}-` : '';
    const env = {
      ...process.env,
      WORKER_NAME_PREFIX: prefix,
      WORKER_NAME: `${namePrefix}confidence-cloudflare-resolver`,
      EVENTS_QUEUE_NAME: `${namePrefix}events-queue`,
    };
    delete env.FLAG_LOG_QUEUE_COUNT;
    if (count !== undefined) env.FLAG_LOG_QUEUE_COUNT = String(count);
    const result = spawnSync('bash', ['-c', `
set -euo pipefail
${validation}
CLOUDFLARE_ACCOUNT_ID=test-account
CLOUDFLARE_API_TOKEN=test-token
curl() {
    if [[ "$*" == *"-X POST"* ]]; then
        echo "CREATE $*" >&2
        printf '{}${createStatus}'
    else
        printf '${existing ? '{"result":[{"id":"existing"}]}200' : '{"result":[]}200'}'
    fi
}
${provisioning}
${configuration}
`], { cwd: directory, env, encoding: 'utf8' });
    return { ...result, config: readFileSync(join(directory, 'wrangler.toml'), 'utf8') };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

for (const count of [undefined, 2, 8]) {
  for (const prefix of ['', 'customer']) {
    for (const existing of [false, true]) {
      test(`count=${count ?? 'default'}, prefix=${prefix || 'none'}, existing=${existing}`, () => {
        const result = run({ count, prefix, existing });
        assert.equal(result.status, 0, result.stderr);
        const expectedCount = count ?? 1;
        const base = `${prefix ? prefix + '-' : ''}flag-logs-queue`;
        const consumers = [...result.config.matchAll(/\[\[queues\.consumers\]\]\s+queue = "([^"]+)"/g)].map(m => m[1]);
        const producers = [...result.config.matchAll(/\[\[queues\.producers\]\]\s+queue = "([^"]+)"\s+binding = "([^"]+)"/g)].map(m => [m[1], m[2]]);
        const expected = Array.from({ length: expectedCount }, (_, i) => i === 0 ? base : `${base}-${i + 1}`);
        assert.deepEqual(consumers.filter(name => name.includes('flag-logs-queue')), expected);
        assert.deepEqual(producers.filter(([, binding]) => binding.startsWith('flag_logs_queue')),
          expected.map((name, i) => [name, i === 0 ? 'flag_logs_queue' : `flag_logs_queue_${i + 1}`]));
        assert.equal(consumers.length, expectedCount + 1);
        assert.equal(producers.length, expectedCount + 1);
        assert(consumers.includes(`${prefix ? prefix + '-' : ''}events-queue`));
        assert.equal((result.stderr.match(/CREATE /g) || []).length, existing ? 0 : expectedCount);
        if (!existing) {
          for (const name of expected) assert(result.stderr.includes(`"queue_name": "${name}"`));
        }
      });
    }
  }
}

for (const count of ['0', '-1', '1.5', 'abc', '01', '10000', '999999999999999999999']) {
  test(`rejects invalid count ${count} before provisioning`, () => {
    const result = run({ count });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /FLAG_LOG_QUEUE_COUNT must be/);
    assert(!result.stderr.includes('CREATE'));
  });
}

test('aborts when queue creation fails', () => {
  const result = run({ count: 2, createStatus: '403' });
  assert.equal(result.status, 1);
  assert.match(result.stdout, /Failed to create queue/);
  assert(!result.config.includes('flag_logs_queue_2'));
});
