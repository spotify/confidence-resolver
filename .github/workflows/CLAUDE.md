# CI/CD Workflows

## CI (`ci.yml`)

CI is a **single `docker build .`** — the multi-stage Dockerfile is the entire test/lint/build pipeline. There are no separate per-component jobs. If the Dockerfile's `all` stage succeeds, CI passes.

- **ARM runners**: `ubuntu-24.04-arm` (not the default x86)
- **Registry-based cache**: Docker layer cache is stored in GHCR (`ghcr.io/<repo>/cache:main`). Cache is only **pushed** on `main` branch pushes. PR builds read from cache but don't update it.

## Release & Publish (`release-please.yml`)

Release Please detects version bumps per component independently. Each component has its own conditional publish job.

### Publish dependency chain

The Rust provider (`openfeature-provider/rust`) depends on `confidence-resolver` being published to crates.io first. The workflow enforces this:

1. `publish-confidence-resolver-release` runs first
2. `publish-rust-provider-release` waits for it, then **sleeps 30 seconds** for crates.io index propagation
3. If only the Rust provider is released (resolver not changed), it skips the wait


All publish jobs require the **`deployment` GitHub environment** (environment protection rules apply).

### Cloudflare deployer

On `confidence-cloudflare-resolver` release, the deployer Docker image is built and pushed to `ghcr.io/<owner>/confidence-cloudflare-deployer` with both a version tag and `latest`.

## PR Lint (`lint-pr-name.yaml`)

PR titles must follow **conventional commits** (`feat:`, `fix:`, `chore:`, etc.). Enforced via `amannn/action-semantic-pull-request@v5`. Posts a sticky comment on the PR explaining the error if validation fails.
