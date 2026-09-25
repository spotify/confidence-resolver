import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { assess, deploymentDecision, flagLogSink, runProbe, validateDeployArgs } from './memory-preflight.mjs';

const MIB = 1024 * 1024;
const sample = (baselineMiB) => ({
  wasmBytes: (baselineMiB - 2) * MIB,
  jsHeapCapacityBytes: 2 * MIB, jsHeapUsedBytes: MIB,
});

test('sink advisory follows Worker configuration and CLI overrides', () => {
  assert.equal(flagLogSink(), 'queue');
  assert.equal(flagLogSink({ FLAG_LOG_SINK: ' BUFFER ' }), 'buffer');
  assert.equal(flagLogSink({ FLAG_LOG_SINK: 'unknown' }), 'queue');
  assert.equal(flagLogSink({}, ['--var', 'FLAG_LOG_SINK:buffer']), 'buffer');
  assert.equal(flagLogSink({ FLAG_LOG_SINK: 'buffer' }, ['--var=FLAG_LOG_SINK:queue']), 'queue');
  assert.equal(flagLogSink({}, ['--var', 'FLAG_LOG_SINK:buffer', '--var=FLAG_LOG_SINK:queue']), 'queue');
  assert.equal(assess({ ...sample(70), flagLogSink: 'buffer' }).flagLogSink, 'buffer');
});

test('alternate artifacts cannot receive a misleading pass', () => {
  validateDeployArgs(['--tag', 'release', '--message=update', '--keep-vars']);
  for (const args of [['other.js'], ['--config', 'other.toml'], ['--env=staging'],
    ['--compatibility-date=2026-01-01'], ['--define', 'X=Y'], ['--tag']]) {
    assert.throws(() => validateDeployArgs(args), /unsupported_deploy_arguments/);
  }
});

test('threshold boundaries use WASM plus JS capacity, not JS usage again', () => {
  assert.equal(assess(sample(79)).status, 'pass');
  assert.equal(assess(sample(80)).status, 'warning');
  assert.equal(assess(sample(95)).status, 'warning');
  assert.equal(assess(sample(96)).status, 'abort_deployment');
  assert.equal(assess(sample(96)).baselineBytes, 96 * MIB);
});

test('force keeps abort signal visible but permits deployment', () => {
  for (const report of [assess(sample(96)), { status: 'abort_deployment', reason: 'measurement_failed' }]) {
    assert.equal(deploymentDecision(report, '').exitCode, 1);
    for (const force of ['1', '0']) {
      const decision = deploymentDecision(report, force);
      assert.equal(decision.exitCode, 0);
      assert.equal(decision.forced, true);
      assert.equal(decision.status, 'abort_deployment');
    }
  }
  assert.equal(deploymentDecision(assess(sample(80)), '').exitCode, 0);
});

test('missing or invalid measurements cannot pass', () => {
  assert.throws(() => assess({}));
  assert.throws(() => assess({ ...sample(60), wasmBytes: NaN }));
  assert.throws(() => assess({ ...sample(60), jsHeapUsedBytes: 3 * MIB }));
});

test('CLI emits safe machine-readable failure and honors force', () => {
  for (const force of ['', '1']) {
    const result = spawnSync(process.execPath, [new URL('./memory-preflight.mjs', import.meta.url).pathname, '.', '--config=other.toml'], {
      env: { ...process.env, SKIP_PREFLIGHT_TEST: 'false', FORCE_DEPLOY: force }, encoding: 'utf8',
    });
    assert.equal(result.status, force ? 0 : 1);
    const report = JSON.parse(result.stdout);
    assert.equal(report.status, 'abort_deployment');
    assert.equal(report.reason, 'unsupported_deploy_arguments');
    assert.equal(report.forced, Boolean(force));
    assert.match(result.stderr, force ? /FORCE_DEPLOY OVERRIDE/ : /DEPLOYMENT BLOCKED/);
  }
});

test('hung probe is terminated by the parent deadline', async () => {
  await assert.rejects(runProbe('/nonexistent-memory-test', 1), /measurement_timeout/);
});

test('unmeasurable artifacts abort unless explicitly forced', () => {
  for (const force of ['', '1']) {
    const result = spawnSync(process.execPath, [new URL('./memory-preflight.mjs', import.meta.url).pathname,
      '/nonexistent-memory-test'], {
      env: { ...process.env, SKIP_PREFLIGHT_TEST: 'false', FORCE_DEPLOY: force }, encoding: 'utf8',
    });
    assert.equal(result.status, force ? 0 : 1);
    const report = JSON.parse(result.stdout);
    assert.equal(report.status, 'abort_deployment');
    assert.equal(report.reason, 'measurement_failed');
    assert.equal(report.forced, Boolean(force));
  }
});

test('decryption script retains CommonJS behavior', () => {
  const result = spawnSync(process.execPath, [new URL('./decrypt_state.js', import.meta.url).pathname], { encoding: 'utf8' });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Usage: node decrypt_state.js/);
  assert.doesNotMatch(result.stderr, /require is not defined/);
});

test('skip defaults to false and only true bypasses the preflight', () => {
  for (const skip of [undefined, 'false', '0', 'true']) {
    const env = { ...process.env, FORCE_DEPLOY: '' };
    if (skip === undefined) delete env.SKIP_PREFLIGHT_TEST;
    else env.SKIP_PREFLIGHT_TEST = skip;
    const result = spawnSync(process.execPath, [new URL('./memory-preflight.mjs', import.meta.url).pathname,
      '/nonexistent-memory-test', '--config=other.toml'], { env, encoding: 'utf8' });
    assert.equal(result.status, skip === 'true' ? 0 : 1);
    const report = JSON.parse(result.stdout);
    assert.equal(report.status, skip === 'true' ? 'skipped' : 'abort_deployment');
    if (skip === 'true') assert.match(result.stderr, /memory preflight skipped/);
  }
});

test('built Worker initializes and reports memory with the selected sink advisory', {
  skip: !process.env.MEMORY_PREFLIGHT_TEST_WORKER_DIR,
}, () => {
  for (const sink of ['queue', 'buffer']) {
    const result = spawnSync(process.execPath, [new URL('./memory-preflight.mjs', import.meta.url).pathname,
      process.env.MEMORY_PREFLIGHT_TEST_WORKER_DIR, '--var', `FLAG_LOG_SINK:${sink}`], {
      env: { ...process.env, FORCE_DEPLOY: '', SKIP_PREFLIGHT_TEST: 'false' }, encoding: 'utf8',
    });
    const report = JSON.parse(result.stdout);
    assert.ok(report.wasmBytes > 0, result.stderr);
    assert.equal(report.status, assess(report).status);
    assert.equal(result.status, deploymentDecision(report, '').exitCode);
    assert.equal(report.flagLogSink, sink);
    assert.equal(result.stderr.includes('FLAG_LOG_SINK=buffer:'), sink === 'buffer');
  }
});
