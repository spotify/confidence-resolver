# Java provider rewrite — plans

The Java provider's rewrite from the embedded rust-guest resolver to state-to-wasm flag modules,
as the Go provider (reference implementation) already did on `origin/go-provider`.

- Every plan here maps to one reviewed PR into `next`, in order.
- The Go implementation on `origin/go-provider` is the normative reference: port its behavior,
  adapt its idioms. Each phase doc lists the Go files and commits it ports.
- Check a phase off here when its PR lands, and record the PR number.

## Phases

| # | Plan | Ports (go commits) | Status |
|---|------|--------------------|--------|
| 0 | [00-architecture.md](00-architecture.md) | — | design |
| 1 | [01-chicory-spike.md](01-chicory-spike.md) | risk gate, no PR | ✅ passed — Endive 1.2.0 |
| 2 | [02-protos-and-fixtures.md](02-protos-and-fixtures.md) | `93c7365e` | ☐ |
| 3 | [03-host-runtime.md](03-host-runtime.md) | `31cd404a` (+ `c85edc44`) | ☐ |
| 4 | [04-cdn-fetcher.md](04-cdn-fetcher.md) | `a1b35aa8` | ☐ |
| 5 | [05-ingest-shipper.md](05-ingest-shipper.md) | `7b2a45ec` | ☐ |
| 6 | [06-engine.md](06-engine.md) | `fe0ddf42` (+ `e821478f`) | ☐ |
| 7 | [07-provider-api.md](07-provider-api.md) | `fb866c6d` (+ `0ce35258`, `870eb772`, `c7f91211`) | ☐ |
| 8 | [08-build-ci-release-docs.md](08-build-ci-release-docs.md) | `42610a23`, `b8df8180` | ☐ |

## Working agreements

- Branches under `nicklasl/`; conventional commits; PRs into `next` opened as drafts.
- Small changes: one component per PR, as the Go sequence did. Nothing is merged from `main`;
  `main` fixes are folded in during the rewrite.
- Java stays out of `PROVIDERS`, the Dockerfile and release-please until phase 8 — until then the
  provider "does not build" on `next`, exactly as the branch's README describes.

## Sources of truth

- `openfeature-provider/proto/` — the state-to-wasm protos (verbatim copies; see its README).
- `openfeature-provider/testdata/` — shared states and compiled modules (`basic`), committed so
  tests need no compiler. Rebuilt with the state-to-wasm compiler; record its revision.
- `origin/go-provider` — the reference implementation. Read with
  `git show origin/go-provider:<path>` until it merges into `next`.
