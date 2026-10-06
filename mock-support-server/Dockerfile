FROM golang:1.26-alpine3.22 AS build
WORKDIR /src/mock-support-server

# Copy module files and download dependencies (cached)
COPY mock-support-server/go.mod ./go.mod
COPY mock-support-server/go.sum ./go.sum
RUN --mount=type=cache,target=/go/pkg/mod \
    go mod download

# Copy sources (protos are already generated and checked in)
COPY mock-support-server/main.go ./main.go
COPY mock-support-server/genproto ./genproto

# Build statically linked binary
RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    CGO_ENABLED=0 go build -o /out/mock-support-server .

FROM alpine:3.24
COPY --from=build /out/mock-support-server /mock-support-server
COPY data/account_id /data/account_id
COPY data/resolver_state_current.pb /data/resolver_state_current.pb
# Single unified port for HTTP+h2c (REST+gRPC)
ENV PORT=8081
EXPOSE 8081
ENTRYPOINT ["/mock-support-server"]

