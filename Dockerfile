# syntax=docker/dockerfile:1

# The CI pipeline. Two stages collect everything that must pass:
#
# - `all` (the default target): build, unit tests and lint. It needs no secrets, so it passes for
#   pull requests from forks.
# - `e2e`: the end-to-end tests against the Confidence backend, which need secrets. CI runs it
#   where the secrets are available.
#
# Each provider adds stages named <component>.<action>, e.g. openfeature-provider-go.test, where
# action is one of build, test, test_e2e, lint, artifact or publish, and wires the ones CI runs
# into `all` (or `e2e`, for test_e2e) by copying a file from each:
#
#   COPY --from=openfeature-provider-go.test /app/go.mod /markers/test-openfeature-go
#
# Shared inputs live in openfeature-provider/proto (state-to-wasm protos) and are copied in by the
# stages that need them.
#
# Secrets are mounted, never written to a layer:
#
#   RUN --mount=type=secret,id=confidence_client_secret \
#       CONFIDENCE_CLIENT_SECRET=$(cat /run/secrets/confidence_client_secret) make test-e2e
#
# CI provides confidence_client_secret and confidence_client_encryption_key to `e2e` only.

# ==============================================================================
# E2E - end-to-end tests against the Confidence backend (needs secrets)
# ==============================================================================
FROM scratch AS e2e

# ==============================================================================
# All - Build and validate everything (default target)
# ==============================================================================
FROM scratch AS all
