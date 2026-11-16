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
# OpenFeature Provider (Go)
# ==============================================================================
FROM golang:1.25-alpine AS openfeature-provider-go.base

RUN apk add --no-cache make

# The repository layout: the tests read openfeature-provider/testdata by relative path.
WORKDIR /src/openfeature-provider/go

COPY openfeature-provider/go/go.mod openfeature-provider/go/go.sum ./
RUN go mod download

COPY openfeature-provider/testdata/ /src/openfeature-provider/testdata/
COPY openfeature-provider/go/Makefile ./
COPY openfeature-provider/go/confidence/ ./confidence/

FROM openfeature-provider-go.base AS openfeature-provider-go.build
RUN make build

FROM openfeature-provider-go.build AS openfeature-provider-go.test
RUN make test

FROM openfeature-provider-go.build AS openfeature-provider-go.lint
RUN make lint

FROM openfeature-provider-go.build AS openfeature-provider-go.test_e2e
RUN --mount=type=secret,id=confidence_client_secret \
    --mount=type=secret,id=confidence_client_encryption_key \
    CONFIDENCE_CLIENT_SECRET=$(cat /run/secrets/confidence_client_secret) \
    CONFIDENCE_CLIENT_ENCRYPTION_KEY=$(cat /run/secrets/confidence_client_encryption_key) \
    make test-e2e

# ==============================================================================
# E2E - end-to-end tests against the Confidence backend (needs secrets)
# ==============================================================================
FROM scratch AS e2e
COPY --from=openfeature-provider-go.test_e2e /src/openfeature-provider/go/go.mod /markers/test-e2e-openfeature-go

# ==============================================================================
# All - Build and validate everything (default target)
# ==============================================================================
FROM scratch AS all
COPY --from=openfeature-provider-go.test /src/openfeature-provider/go/go.mod /markers/test-openfeature-go
COPY --from=openfeature-provider-go.lint /src/openfeature-provider/go/go.mod /markers/lint-openfeature-go
