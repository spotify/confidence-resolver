# Java provider rewrite — plans

The Java provider's rewrite from the embedded rust-guest resolver to state-to-wasm flag modules,
as the Go provider (reference implementation) already did on `next` (merged as #700).

- The rewrite lands as **one reviewed PR into `next`**, like the Go provider's #700: each phase is
  a commit on the branch, reviewed as a stack. Per-commit buildability is not required — the
  merged state builds and tests green.
- The Go implementation on `next` is the normative reference: port its behavior, adapt its
  idioms. Each phase doc lists the Go files and commits it ports.
- Check a phase off here when the rewrite PR lands, and record the PR number.

## Phases

| # | Plan | Ports (go commits) | Status |
|---|------|--------------------|--------|
| 0 | [00-architecture.md](00-architecture.md) | — | design |
| 1 | [01-chicory-spike.md](01-chicory-spike.md) | risk gate, no PR | ✅ passed — Endive 1.2.0 |
| 2 | [02-protos-and-fixtures.md](02-protos-and-fixtures.md) | `93c7365e` | ✅ done |
| 3 | [03-host-runtime.md](03-host-runtime.md) | `31cd404a` (+ `c85edc44`) | ✅ done |
| 4 | [04-cdn-fetcher.md](04-cdn-fetcher.md) | `a1b35aa8` | ✅ done |
| 5 | [05-ingest-shipper.md](05-ingest-shipper.md) | `7b2a45ec` | ✅ done |
| 6 | [06-engine.md](06-engine.md) | `fe0ddf42` (+ `e821478f`) | ✅ done |
| 7 | [07-provider-api.md](07-provider-api.md) | `fb866c6d` (+ `0ce35258`, `870eb772`, `c7f91211`) | ☐ |
| 8 | [08-build-ci-release-docs.md](08-build-ci-release-docs.md) | `42610a23`, `b8df8180` | ☐ |

## Working agreements

- Branches under `nicklasl/`; conventional commits; PRs into `next` opened as drafts.
- Small changes: one component per commit, as the Go sequence did. Nothing is merged from `main`;
  `main` fixes are folded in during the rewrite.
- Java stays out of `PROVIDERS`, the Dockerfile and release-please until the build commit (phase
  8) of the rewrite PR — until then the provider "does not build" on `next`, exactly as the
  branch's README describes. The old source stays until the provider commit (phase 7) removes it,
  as Go's merge did.

## Sources of truth

- `openfeature-provider/proto/` — the state-to-wasm protos (verbatim copies; see its README).
- `openfeature-provider/testdata/` — shared states and compiled modules (`basic`), committed so
  tests need no compiler. Rebuilt with the state-to-wasm compiler; record its revision.
- `next` — the reference implementation (merged as #700; see
  `openfeature-provider/go`).
