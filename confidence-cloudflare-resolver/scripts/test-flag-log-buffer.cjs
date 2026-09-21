#!/usr/bin/env node
// Run the real Rust buffer with an injected clock and queue publisher.
const { spawnSync } = require('node:child_process');
const { resolve } = require('node:path');

const root = resolve(__dirname, '../..');
const scenarios = {
  burst: ['Statistics burst preserves counts and reduces publishes', [
    'statistics_burst_aggregates_into_one_publish',
  ]],
  piggyback: ['Exposure bypasses timer and carries buffered statistics', [
    'assignment_wakes_pending_timer',
    'aggregates_telemetry_into_assignment_message',
  ]],
  silence: ['A lone request flushes without further traffic', [
    'single_statistics_request_flushes_after_silence',
  ]],
  exposures: ['Exposure-only traffic never waits for the batching timer', [
    'exposure_only_traffic_publishes_without_batching_delay',
  ]],
  recovery: ['Failed publishes retry, recover, and retain data on cancellation', [
    'failed_publish_recovers_without_losing_counts',
    'failures_retry_on_timer_then_retain_and_release_scheduler',
    'cancellation_during_publish_retains_batch_and_releases_scheduler',
    'failed_or_cancelled_send_is_retained_without_losing_new_arrivals',
  ]],
  priority: ['Exposures take priority over statistics and their retries', [
    'exposure_interrupts_statistics_retry_backoff',
    'exposure_overtakes_failed_statistics_without_discarding_them',
    'full_statistics_buffer_does_not_flush_early_and_yields_capacity_to_exposures',
    'statistics_cannot_displace_exposures',
  ]],
  limits: ['Size bounds and empty assignments', [
    'bounds_memory_and_encoded_message_size_including_in_flight',
    'empty_assignment_does_not_trigger_early_flush',
    'timer_flushes_without_another_request_and_merges_arrivals',
  ]],
  config: ['Deployer opt-in, disabled, and invalid configuration (not runtime bypass)', []],
};

const args = process.argv.slice(2);
if (args.includes('--help') || args.includes('--list')) {
  console.log('Usage: node confidence-cloudflare-resolver/scripts/test-flag-log-buffer.cjs [scenario ...]');
  console.log('No arguments runs all scenarios. Requires Cargo and Node.js; no deployment or credentials.');
  for (const [name, [description]] of Object.entries(scenarios)) console.log(`  ${name}: ${description}`);
  process.exit(0);
}
const selected = args.length ? args : Object.keys(scenarios);
for (const name of selected) {
  if (!Object.hasOwn(scenarios, name)) {
    console.error(`Unknown scenario: ${name}. Use --list.`);
    process.exit(2);
  }
}

function run(command, commandArgs, expectedTest) {
  const result = spawnSync(command, commandArgs, { cwd: root, encoding: 'utf8' });
  if (result.error || result.status !== 0 ||
      (expectedTest && !result.stdout.includes(`test ${expectedTest} ... ok`))) {
    process.stderr.write(result.stdout || '');
    process.stderr.write(result.stderr || '');
    throw new Error(result.error?.message || `Command failed or expected test did not run: ${command} ${commandArgs.join(' ')}`);
  }
}

console.log('Local buffer scenarios: real aggregation/scheduler, fake clock and publisher.');
console.log('Does not test Cloudflare isolate lifetime, waitUntil, or actual queue delivery.');
try {
  for (const name of selected) {
    const [description, tests] = scenarios[name];
    console.log(`\nRUN ${name}: ${description}`);
    if (name === 'config') {
      run(process.execPath, ['--test', '--test-name-pattern=flag log buffer option:',
        'confidence-cloudflare-resolver/deployer/script.test.cjs']);
    }
    for (const test of tests) {
      const fullName = `flag_log_buffer::tests::${test}`;
      run('cargo', ['test', '-p', 'confidence-cloudflare-resolver', '--lib', fullName,
        '--', '--exact', '--color', 'never'], fullName);
    }
    console.log(`PASS ${name}`);
  }
  console.log(`\n${selected.length} scenario groups passed.`);
} catch (error) {
  console.error(error.message);
  process.exit(1);
}
