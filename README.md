# Confidence OpenFeature Providers

> [!NOTE]
> This is the `next` branch. It replaces the in-repo Rust resolver with state-to-wasm: each
> credential's flags are compiled to a WebAssembly module, which the providers fetch from a CDN and
> run locally. Providers are rewritten here one at a time and released as release candidates
> (e.g. `openfeature-provider/go/v0.30.0-rc.1`). When all are done, `next` replaces `main`.
>
> For released versions, see [`main`](https://github.com/spotify/confidence-resolver/tree/main).

## Repository layout

- `openfeature-provider/`: the OpenFeature providers
  - [Go](./openfeature-provider/go/README.md)
  - [Java](./openfeature-provider/java/README.md)
  - [JavaScript/TypeScript](./openfeature-provider/js/README.md)
  - [Python](./openfeature-provider/python/README.md)
  - [Rust](./openfeature-provider/rust/README.md)
  - [Ruby](./openfeature-provider/ruby/README.md)
- `openfeature-provider/proto/`: the state-to-wasm protos the providers share (verbatim copies)
- `confidence-cloudflare-resolver/`: the Cloudflare Worker resolver

Providers that haven't been rewritten yet are kept as API reference and say so at the top of their
README; they don't build on this branch.

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
