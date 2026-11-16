# Confidence OpenFeature Providers — `next`

> [!IMPORTANT]
> This is the long-lived `next` branch, where the providers are being rebuilt. For released
> versions, use [`main`](https://github.com/spotify/confidence-resolver/tree/main).

## Goal

Move every Confidence OpenFeature provider from the in-repo Rust resolver to **compiled flag
modules**.

Today each provider embeds one generic resolver (Rust compiled to WebAssembly) and feeds it the
account's full resolver state. On `next`, the backend compiles each credential's flags into a
WebAssembly module of its own: it holds only that client's flags and segments, and resolves them
as straight-line code. Providers fetch their module from a CDN, run it in a wasm runtime, and
swap in new modules as flags change.

For providers this means:

- **One small host per language.** A provider loads modules, calls them and ships their logs. The
  resolver logic, and the state format, live in the modules.
- **Faster, leaner resolves.** Measured with the in-progress Go provider on production-shaped
  data: about 4–5× faster per evaluation than `main`, with about 10× less allocation.
- **Less state in memory.** A client holds its own flags, not the whole account's.

## How the branch works

- Providers are rewritten one at a time, each as a reviewed PR into `next`. Until its rewrite
  lands, a provider is kept as API reference: its source stays, it doesn't build, and its README
  says so.
- Rewritten providers are released as release candidates from `next`
  (e.g. `openfeature-provider/go/v0.30.0-rc.1`), so they can be tried before GA.
- Nothing is merged from `main`. Provider fixes made on `main` are carried over when that provider
  is rewritten.
- When every provider is done, `next` replaces `main` and the GA releases ship from there.

## Status

| Provider | Status |
|----------|--------|
| [Go](./openfeature-provider/go/README.md) | Rewrite in progress |
| [Java](./openfeature-provider/java/README.md) | Reference, pending rewrite |
| [JavaScript/TypeScript](./openfeature-provider/js/README.md) | Reference, pending rewrite |
| [Python](./openfeature-provider/python/README.md) | Reference, pending rewrite |
| [Ruby](./openfeature-provider/ruby/README.md) | Reference, pending rewrite |
| [Rust](./openfeature-provider/rust/README.md) | Reference, pending rewrite |
| [Cloudflare resolver](./confidence-cloudflare-resolver/README.md) | Reference, pending rewrite |

Not on `next` yet: `track()` (event tracking), which will return through the flag modules.

## Repository layout

- `openfeature-provider/<lang>/`: the providers
- `openfeature-provider/proto/`: the module protos providers share (verbatim copies; see its README)
- `confidence-cloudflare-resolver/`: the Cloudflare Worker resolver

## Development

```bash
docker build .   # build, test and lint every rewritten provider (what CI runs)
make             # the same with local tools
```

E2E tests need Confidence credentials, passed as Docker secrets:

```bash
docker build \
  --secret id=confidence_client_secret,src=<file> \
  --secret id=confidence_client_encryption_key,src=<file> \
  .
```

## License

See `LICENSE` for details.
