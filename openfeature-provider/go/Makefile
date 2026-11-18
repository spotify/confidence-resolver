# Go OpenFeature provider. Targets follow the provider Makefile contract (see the root CLAUDE.md).

.PHONY: build test test-e2e lint clean proto

build:
	go build ./...

# Unit tests (no credentials needed).
test:
	go test -skip E2E ./...

# End-to-end tests against the module CDN and ingestion.
test-e2e:
	@test -n "$$CONFIDENCE_CLIENT_SECRET" -a -n "$$CONFIDENCE_CLIENT_ENCRYPTION_KEY" || \
		(echo "CONFIDENCE_CLIENT_SECRET and CONFIDENCE_CLIENT_ENCRYPTION_KEY must be set" && exit 1)
	go test -count=1 -run E2E ./...

lint:
	@test -z "$$(gofmt -l . | tee /dev/stderr)" || (echo "Files need formatting: gofmt -w ." && exit 1)
	go vet ./...

clean:
	go clean -testcache

# Regenerate confidence/internal/s2wpb from ../proto.
proto:
	./scripts/generate_s2w_proto.sh

.DEFAULT_GOAL := build
