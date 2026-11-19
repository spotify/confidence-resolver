# 04 — Module CDN fetcher

Ports go commit `a1b35aa8`. One commit of the rewrite PR. Reference:
`internal/cdn/fetcher.go` on `next` (merged as #700).

**Status: ✅ landed** — `CdnFetcher` + `CdnFetcherTest` (ETag flow, decryption,
classification, key parsing) and `CdnFetcherE2ETest` (self-skipping without credentials),
verified in the harness; 27 tests green, 1 e2e skipped.

## Goal

`com.spotify.confidence.sdk.internal.cdn.CdnFetcher` fetches the credential's module from the
module CDN, decrypts it, and answers a fetch outcome the engine's poll loop can branch on.

## Behavior

- `GET {base}/modules/{sha256(clientSecret) as lowercase hex}.wasm.enc`, base default
  `https://confidence-resolver-module-cdn.spotifycdn.com`, overridable for tests.
- Conditional request: `If-None-Match: <etag>` **only after the caller accepted the module** that
  ETag came with (`accept(etag)`); an unaccepted ETag is never reused, so a module that failed to
  install is fetched in full again.
- Decryption: artifact is `12-byte nonce ‖ ciphertext ‖ 16-byte GCM tag`, AES-256-GCM, empty
  associated data; key = the provider's 64-hex-char encryption key (`parseKey`); plaintext parses
  as `ClientModule{module, log_token}`. The log token is a bare JWT — never logged, never in
  errors.
- Outcomes:
  - `UNCHANGED` (HTTP 304),
  - `MODULE(ClientModule, etag)`,
  - retryable: 404 (not yet provisioned), 408/429/5xx, network errors — the engine's poll loop
    retries at its own cadence,
  - terminal: any other 4xx (bad credential etc.), decryption failure, undecodable artifact —
    maps to fatal provider initialization when it's the first fetch.
- Java side: `java.net.http.HttpClient` with an injectable client (keep the public
  `HttpClientFactory` seam the config already has, or a package-private one — decided in phase 7);
  10s request timeout, no redirects; single request, no internal retry loop (the engine owns
  retry cadence).

## Tests (port `fetcher_test.go`)

- ETag flow: first fetch full; 304 → `UNCHANGED`; accept then 304 → `UNCHANGED`; **reject** then
  next fetch sends **no** `If-None-Match`.
- Status classification: 404/408/429/500 retryable; 401/403/400 terminal; each with the mapped
  error cause.
- Decryption: round-trip of a published fixture through the fake CDN (phase 5's `testutil`, or a
  local helper here); wrong key → terminal; truncated artifact → terminal.
- Key validation: exactly 64 hex chars, case-insensitive, odd/short/binary rejected.

## Done when

The fetcher is fully covered by unit tests against a local HTTP server and no production URL is
hard-coded anywhere except the default constant.

## Deltas from the Go reference (as landed)

- Go's error values map to an exception taxonomy the engine can branch on:
  `CdnException` (retryable base), `NotModifiedException`, `NotPublishedException`
  (retryable: 404 while the credential has no module), and `TerminalCdnException`
  (rejected credential, undecodable artifact). Errors never include key material.
- The fetched artifact is `CdnFetcher.FetchedModule(wasm, logToken, etag)`, renamed from Go's
  `Module` to avoid the host's `Module`.
- The transport is `java.net.http.HttpClient` (injectable); classification, the ETag rules
  (accepted only via `accept`), the `nonce ‖ ct ‖ tag` layout with empty AAD, and the
  `sha256(clientSecret)` path are faithful to the Go fetcher.
- The e2e test self-skips without credentials via `@EnabledIfEnvironmentVariable` and carries
  the `*E2ETest` suffix; the Makefile's `test` excludes that suffix and `test-e2e` runs only it
  (`mvn test -Dtest='**/*E2ETest'` — failsafe only picks up `*IT`, so the e2e set is surefire-run
  by name, like Go's `-run E2E`).
