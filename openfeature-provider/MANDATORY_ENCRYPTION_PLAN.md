# Mandatory encryption for local providers

Status: implemented breaking API change; release pending.

Previously, local providers accepted an omitted encryption key and fetched plaintext
state. This change requires a key in JavaScript, Java, Go, Python, and Rust. Ruby resolves
remotely and is outside this change.

## Required behavior

- Reject missing, empty, non-hexadecimal, or incorrectly sized keys before network
  calls, WASM allocation, or background tasks. The documented AES-256 key is 32
  bytes, represented by exactly 64 hexadecimal characters. Accept either case;
  do not trim whitespace or include the supplied value in error messages.
- Make the key required in public types and constructor signatures where possible.
  Keep runtime validation for dynamic callers and languages with zero values.
  No public convenience constructor may create a provider without an encryption
  key, whether supplied directly or through validated configuration.
- Request only `/<sha256(client-secret)>.enc` and always authenticate/decrypt the
  response. Remove plaintext fallback; failures must never retry the plaintext URL.
- Preserve existing initialization and stale-state behavior for network or
  authentication failures. Invalid configuration fails at construction; a
  well-formed but incorrect key fails when state is decrypted.
- Keep the existing initialization telemetry label with `encryption=true` so
  dashboards can still distinguish these releases from older providers.

## Public API changes

| Provider | Required API | Failure behavior |
|----------|--------------|------------------|
| JavaScript | Required `ProviderOptions.encryptionKey: string` | Constructor throws for invalid configuration, including untyped callers |
| Python | Required `encryption_key: str` argument after `client_secret` | Omission raises `TypeError`; invalid values raise `ValueError` |
| Rust | `ProviderOptions::new(client_secret, encryption_key)` and non-optional `String` field | `ConfidenceProvider::new` returns `Error::Configuration` for invalid keys |
| Go | Required, documented `ProviderConfig.EncryptionKey` field | `NewProvider` returns an error for invalid or zero-value keys |
| Java | Key-taking convenience constructors; builder validates the key on `build()` | Invalid configuration raises `IllegalArgumentException` before constructing transport resources |

For Java, replace keyless `LocalProviderConfig` constructors with key-taking
equivalents, and require a key in convenience provider constructors. Keep the
existing `.encryptionKey(...)` builder setter. For Rust, retain
`.with_encryption_key(...)` only as an explicit override if useful; it must no
longer be the only way to supply the required constructor argument.

State-fetcher APIs also need review: remove optional key parameters and keyless
constructors used for CDN fetching. Test-only injection of already decoded state
must remain internal to tests and must not expose a public keyless constructor or
provide a production CDN fallback.

## Implementation scope

Keep each provider's change in its own commit, including that provider's API,
fetcher, tests, examples, and documentation updates. Put shared mock-server and
cross-provider documentation changes in separate commits; land shared fixture
support before provider commits that depend on it.

1. Update provider configuration and constructor validation in all five languages.
2. Make CDN fetchers always use encrypted URLs and authenticated decryption.
3. Migrate unit fixtures to encrypted responses using shared test helpers. Use
   the existing test key and encrypted fixture where applicable; do not mock
   decryption in tests intended to verify the encrypted fetch path.
4. Update E2E and flag-log suites to require `CONFIDENCE_CLIENT_ENCRYPTION_KEY`.
   Remove the successful unencrypted-provider test cases.
5. Update demos, benchmarks, README examples, JS migration and React integration
   examples, and the shared integration guide. Keep keys in backend configuration.
6. Update `mock-support-server`: its CDN handler currently requires a 64-character
   path and rejects `.enc`. Support encrypted upstream responses and encrypt local
   fixtures with a configured test key. Keep benchmark client/server keys aligned.

These changes are in host providers and supporting fixtures. A resolver WASM
rebuild is necessary only if core resolver or WASM source changes become necessary.

## Verification

- Across all five providers: missing, empty, whitespace-only, malformed hex, and
  wrong-length keys fail before transport/resource creation; valid uppercase and
  lowercase keys are accepted.
- Valid encrypted state initializes and resolves flags. A wrong key, tampered
  ciphertext, truncated response, and plaintext response fail without a plaintext
  request. Check both initial fetch and refresh behavior.
- Encrypted polling preserves conditional requests and `304` handling. Decryption
  failures must not incorrectly advance the cached ETag and prevent a retry.
- Initialization telemetry reports `encryption=true`; resolution, exposure,
  assignment, and event tests still pass with encrypted state.
- Run each affected provider's lint, build, and unit tests, followed by encrypted
  E2E tests when credentials are available. Smoke-test benchmarks against the
  updated mock server.

## Migration and release

Users can prepare on existing releases by supplying the encryption key alongside
the client secret. The integration guide records encryption support starting in
JS/Java 0.16.0, Go 0.19.0, Python 0.9.0, and Rust 0.7.0.

To obtain the encryption key, open the [Confidence Admin clients page](https://app.confidence.spotify.com/admin/clients),
select the client, and locate the credential used by the provider. The encryption
key is accessible alongside that credential and is unique to each client
credential; use the key belonging to the same credential as the configured client
secret. Include these directions in each provider's migration documentation and
release notes. Deploy the configuration and verify encrypted initialization before
upgrading to a release that requires it.

Use breaking conventional commits for each provider, for example
`feat(js)!: require encryption keys`, and the draft PR title:

`feat!: require encryption keys for local providers`

Include a `BREAKING CHANGE:` footer describing required arguments, early
validation, and removal of plaintext fetching. Ensure each affected package's
release notes contains its migration instructions. Do not manually change
generated changelogs or version manifests as part of implementation.

The repository currently uses `bump-minor-pre-major: true`, and all five providers
are at `0.x`. Treat this as a breaking API release under that existing versioning
policy; moving to `1.0` is a separate release decision. Inspect Release Please's
generated versions and notes before publishing.

Requiring keys in new SDK releases does not itself disable plaintext CDN access
for existing SDK versions. Any server-side retirement needs a separately announced
timeline and migration policy. After release, update the internal SDK version
registries as required by the development guide.

## Implementation verification

- Python: 121 unit tests passed; Ruff and mypy passed; source distribution and
  wheel built successfully. The source distribution now explicitly includes its
  generated WASM assets so a wheel can be built from it.
- Go: unit tests and `go vet` passed; demo and benchmark compiled; the benchmark
  initialized from encrypted local state and completed with zero evaluation errors.
- Rust: 113 unit tests passed, one existing test ignored; examples and E2E tests
  compiled; production-library Clippy passed. The benchmark's existing signal
  handler required enabling Tokio's `signal` feature in dev dependencies.
- Java: 106 unit tests passed; additional wrong-key coverage and package build
  passed; Java formatting applied.
- JavaScript: 200 unit tests passed; TypeScript checking and the locked dependency
  build passed; the benchmark initialized from encrypted local state and completed
  with zero evaluation errors. Formatting passed for changed source files.
- Shared mock server: compiled successfully and served authenticated encrypted
  fixtures to both Go and JavaScript benchmarks.

Live-backend E2E suites were not run because client credentials were unavailable.
Go and Java have some live-backend suites named as unit tests; those were excluded
from the unit runs above. Repository-wide JS formatting still reports the existing
`AGENTS.md` documentation alias, and Rust's broader all-target Clippy run reports
existing approximate-PI literals in unrelated tests; production-library lint passed.

No core resolver or WASM source was changed. Tests used the WASM artifacts already
committed with the Go provider; no binary rebuild or binary commit was required.
