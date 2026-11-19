# openfeature-provider/java Makefile
# Targets follow the provider Makefile contract (see the root CLAUDE.md).

.PHONY: build test test-e2e clean proto

build:
	mvn -q package -DskipTests

# Unit tests (no credentials needed).
test:
	mvn -q test -Dtest='!**/*E2ETest'

# End-to-end tests against the module CDN and ingestion.
test-e2e:
	@test -n "$$CONFIDENCE_CLIENT_SECRET" -a -n "$$CONFIDENCE_CLIENT_ENCRYPTION_KEY" || \
		(echo "CONFIDENCE_CLIENT_SECRET and CONFIDENCE_CLIENT_ENCRYPTION_KEY must be set" && exit 1)
	mvn -q test -Dtest='**/*E2ETest'

clean:
	mvn -q clean

# Regenerate the state-to-wasm protobuf classes from ../proto.
proto:
	mvn -q protobuf:compile

.DEFAULT_GOAL := build
