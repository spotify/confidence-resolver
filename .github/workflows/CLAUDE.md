# CI/CD Workflows

## CI (`ci.yml`)

CI builds two Dockerfile stages; the multi-stage Dockerfile is the entire pipeline, with no per-component jobs.

- **`Build & Test`** builds `all`: build, unit tests and lint. It needs no secrets, so it passes for PRs from forks. This is the check to require.
- **`E2E`** builds `e2e`: the end-to-end tests, with the Confidence secrets mounted. It runs on pushes, manual runs and PRs from branches in this repository; for PRs from forks (which get no secrets) it is skipped. Never give fork code the secrets (e.g. via `pull_request_target`); a maintainer runs the e2e tests for a fork PR by pushing its commits to a branch here.

- **ARM runners**: `ubuntu-24.04-arm` (not the default x86)
- **Registry-based cache**: Docker layer cache is stored in GHCR, one per long-lived branch (`ghcr.io/<repo>/cache:main`, `cache:next`). Only `Build & Test` on branch pushes writes the cache; PR builds and `E2E` read it.

## Release & Publish (`release-please.yml`)

Release Please runs on pushes to `main` and `next` and releases from the pushed branch, using that
branch's `release-please-config.json` and manifest.

On `next`, the config lists only the providers rewritten on state-to-wasm modules and releases
release candidates (`versioning: prerelease`, `prerelease-type: rc`), e.g.
`openfeature-provider/go/v0.30.0-rc.1`. Publish jobs come back with each provider's rewrite; they
run in the **`deployment` GitHub environment**. npm releases must publish rcs under a dist-tag
(`--tag next`) so they don't become `latest`.

`bootstrap-sha` is the main/next branch point, so changelogs only collect `next`'s commits.

- **Java**: `publish-java-provider-release` runs when Release Please cuts a java release. It checks
  out the release tag and builds the Dockerfile's `openfeature-provider-java.publish` stage, which
  signs and `mvn deploy`s the rc to Maven Central. The Maven settings and GPG key and passphrase
  are mounted as Docker secrets.

### Post-release

After a release is published, remember to update internal version registries with the new version numbers.

## PR Lint (`lint-pr-name.yaml`)

PR titles must follow **conventional commits** (`feat:`, `fix:`, `chore:`, etc.). Enforced via `amannn/action-semantic-pull-request@v6`. Posts a sticky comment on the PR explaining the error if validation fails.
