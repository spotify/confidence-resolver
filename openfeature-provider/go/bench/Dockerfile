FROM golang:1.25-alpine3.22 AS build
WORKDIR /src

# Prepare module files in correct relative locations to satisfy the replace directive
COPY openfeature-provider/go/bench/go.mod openfeature-provider/go/bench/go.sum ./openfeature-provider/go/bench/
COPY openfeature-provider/go/go.mod openfeature-provider/go/go.sum ./openfeature-provider/go/

# Prime module cache
RUN --mount=type=cache,target=/go/pkg/mod \
    go mod download -C ./openfeature-provider/go/bench

# Copy sources (bench + local provider, including embedded wasm)
COPY openfeature-provider/go ./openfeature-provider/go
COPY openfeature-provider/go/bench ./openfeature-provider/go/bench

# Build static bench binary
RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    CGO_ENABLED=0 \
    go build -C ./openfeature-provider/go/bench -o /out/bench .

FROM alpine:3.22
RUN apk add --no-cache ca-certificates
COPY --from=build /out/bench /bench
ENTRYPOINT ["/bench"]


