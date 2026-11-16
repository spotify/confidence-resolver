# CI/CD Workflows

## CI (`ci.yml`)

CI is a **single `docker build .`** — the multi-stage Dockerfile is the entire test/lint/build pipeline. There are no separate per-component jobs. If the Dockerfile's `all` stage succeeds, CI passes.

- **ARM runners**: `ubuntu-24.04-arm` (not the default x86)
- **Registry-based cache**: Docker layer cache is stored in GHCR, one per long-lived branch (`ghcr.io/<repo>/cache:main`, `cache:next`). Cache is only **pushed** on branch pushes. PR builds read their base branch's cache but don't update it.

## Release & Publish (`release-please.yml`)

Release Please runs on pushes to `main` and `next` and releases from the pushed branch, using that
branch's `release-please-config.json` and manifest.

On `next`, the config lists only the providers rewritten on state-to-wasm modules and releases
release candidates (`versioning: prerelease`, `prerelease-type: rc`), e.g.
`openfeature-provider/go/v0.30.0-rc.1`. Publish jobs come back with each provider's rewrite; they
run in the **`deployment` GitHub environment**. npm releases must publish rcs under a dist-tag
(`--tag next`) so they don't become `latest`.

### Post-release

After a release is published, remember to update internal version registries with the new version numbers.

## PR Lint (`lint-pr-name.yaml`)

PR titles must follow **conventional commits** (`feat:`, `fix:`, `chore:`, etc.). Enforced via `amannn/action-semantic-pull-request@v6`. Posts a sticky comment on the PR explaining the error if validation fails.
