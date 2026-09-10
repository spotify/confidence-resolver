# Confidence Resolver Server

A native Rust HTTP/gRPC server packaged as a Docker image. One process holds a full
account snapshot and resolves requests from every client in that account. It uses
the existing Rust evaluator directly; Java and a WASM runtime are not required.

## Build and run

From the repository root:

```sh
docker build --target confidence-resolver-server.runtime -t confidence-resolver-server:local .
docker run --rm --name confidence-resolver \
  -p 8090:8090 -p 5990:5990 \
  --env-file /path/to/confidence.env \
  confidence-resolver-server:local
```

The usual configuration uses API-client credentials with access to the account's
resolver state and flag-log APIs. Put these in the environment file:

```dotenv
CONFIDENCE_CLIENT_ID=<api-client-id>
CONFIDENCE_CLIENT_SECRET=<api-client-secret>
```

The server obtains an access token, discovers the signed account-state URL, and
refreshes the snapshot periodically. Each resolve/apply request supplies its own
**flag-client secret**. Bootstrap credentials and flag-client credentials serve
different purposes.

Alternatively, supply a plaintext protobuf `ResolverState` URL:

```dotenv
CONFIDENCE_RESOLVER_STATE_URL=https://example.com/account-state.pb
CONFIDENCE_ACCOUNT=accounts/example
```

For an encrypted `ResolverServiceState` envelope, set the URL and
`CONFIDENCE_RESOLVER_STATE_ENCRYPTION_KEY` (a 32-byte AES-GCM key encoded as 64 hex
characters). The account comes from the envelope; `CONFIDENCE_ACCOUNT`, when set,
is checked against it. API-client credentials are optional in direct-URL mode.
If present, their account must match the loaded state.

Without API-client credentials, flag reports use the envelope's dedicated log
credential, falling back to each request's flag-client secret. Sidecar metadata
reporting requires API-client credentials. Envelopes targeting non-edge log
storage are rejected.

## Routes

HTTP listens on port 8090:

| Method | Path | Purpose |
| --- | --- | --- |
| POST | `/v1/flags:resolve` | Resolve flags; supports immediate or deferred apply |
| POST | `/v1/flags:apply` | Apply flags using a resolve token |
| GET, POST | `/v1/health` | 200 after the first successful state load; otherwise 503 |
| GET | `/v1/metrics` | Prometheus metrics |
| GET | `/v1/telemetry` | Same Prometheus metrics |

```sh
curl http://localhost:8090/v1/flags:resolve \
  -H 'Content-Type: application/json' \
  -d '{"clientSecret":"<flag-client-secret>","evaluationContext":{"targeting_key":"user-123"},"apply":true}'
```

gRPC listens on port 5990 and exposes
`confidence.flags.resolver.v1.FlagResolverService` (`ResolveFlags`, `ApplyFlags`),
standard gRPC health, and v1/v1alpha reflection. Both transports share the same
snapshot and reporting queue. TLS termination for inbound traffic belongs to the
deployment's proxy; outbound Confidence connections use TLS by default.

## Configuration

| Environment variable | Default | Purpose |
| --- | --- | --- |
| `CONFIDENCE_CLIENT_ID`, `CONFIDENCE_CLIENT_SECRET` | unset | API-client bootstrap and account reporting |
| `CONFIDENCE_RESOLVER_STATE_URL` | discovered | Direct account-state URL |
| `CONFIDENCE_ACCOUNT` | discovered | Account ID or `accounts/<id>`; required for direct plaintext state |
| `CONFIDENCE_RESOLVER_STATE_ENCRYPTION_KEY` | unset | Hex AES-256-GCM key for encrypted envelopes |
| `CONFIDENCE_DOMAIN` | `edge-grpc.spotify.com` | Outbound gRPC host, optionally with port |
| `CONFIDENCE_GRPC_PLAINTEXT` | `false` | Allow plaintext outbound gRPC for local testing |
| `CONFIDENCE_RESOLVER_API_URL` | `https://resolver.confidence.dev` | Remote materializations and client-authenticated reporting |
| `CONFIDENCE_RESOLVER_HTTP_PORT` | `8090` | HTTP listen port |
| `CONFIDENCE_RESOLVER_GRPC_PORT` | `5990` | gRPC listen port |
| `CONFIDENCE_RESOLVER_POLL_INTERVAL_SECONDS` | `30` | State polling interval; ETags supported |
| `CONFIDENCE_ASSIGN_LOG_INTERVAL_SECONDS` | `10` | Report flush/retry interval |
| `CONFIDENCE_ASSIGN_LOG_CAPACITY` | `33554432` | Maximum buffered report bytes, including in-flight batches |
| `CONFIDENCE_METADATA_REPORT_INTERVAL_SECONDS` | `300` | Sidecar metadata reporting interval |
| `CONFIDENCE_SEND_APPLY_LOGS` | `true` | Enable assignments and context-value samples |
| `RUST_LOG` | `confidence_resolver_server=info` | Log filter |

Setting `CONFIDENCE_SEND_APPLY_LOGS=false` preserves resolve/apply validation,
resolve tokens, schema discovery and resolve counts. Samples are limited to fields
explicitly marked non-PII, with minimum population and frequency thresholds and a
bounded cache. There are no HLL sketches, distinct-count reports, or Dropwizard
metrics. Materializations always use the remote API and the requesting client's
credentials. There is no custom materialization-store interface.

A failed refresh keeps the last good snapshot and readiness. Materialization read
failure fails that request; write failure is counted and logged while the resolved
result is returned. Reports retry within a bounded in-memory queue and are dropped
with a metric when full. They are not durable across process failure. SIGTERM stops
accepting requests, drains work, and attempts a final report flush within 30 seconds;
allow at least 35 seconds in the deployment's termination grace period.

The runtime image runs as UID/GID 65532 and supports a read-only filesystem. Its
built-in health check calls `/v1/health`. It builds for Docker's selected platform;
use `--platform linux/amd64` or `--platform linux/arm64` as appropriate. Registry
publishing and deployment manifests are outside this initial implementation.

Resolve tokens use a server-specific AES-GCM envelope bound to the account and
request credential. They survive snapshot refreshes and work across Rust replicas
using the same credentials. Callers holding a flag-client secret can also derive
its token key; this binding prevents token reuse under another credential, and is
not proof that a trusted server issued the token. Do not route a deferred apply from
a Java-issued token to this server without separately verifying compatibility.

## Validate

```sh
cargo test --locked -p confidence-resolver-server
docker build --target confidence-resolver-server.test .
docker build --target confidence-resolver-server.lint .
```

The tests use synthetic multi-client state and local HTTP/gRPC backends. They cover
client isolation, credential revocation, refresh failure, encrypted envelopes,
authenticated bootstrap, remote materializations, deferred apply, report retries,
queue limits, and sampling thresholds. No production credentials are required.

To smoke-test the actual image (Python 3 and Docker; `grpcurl` enables gRPC checks):

```sh
cargo run --locked -p confidence-resolver-server --example encode_state -- \
  confidence-resolver-server/tests/fixtures/account.json > /tmp/account-state.pb
python3 confidence-resolver-server/tests/smoke.py /tmp/account-state.pb
```
