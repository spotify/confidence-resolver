# openfeature-provider/java Makefile
# Targets follow the provider Makefile contract (see the root CLAUDE.md).

.PHONY: build test test-e2e lint clean proto

build:
	mvn -q verify -DskipTests
	@j="$$(ls target/openfeature-provider-local-*.jar | grep -v original | head -1)"; \
	jar tf "$$j" | grep -q 'com/spotify/confidence/sdk/shaded/run/endive/' \
		|| (echo "shading broken: run.endive not relocated in $$j" && exit 1); \
	if jar tf "$$j" | grep -qE '^run/endive/'; then \
		echo "shading broken: unrelocated run.endive classes in $$j"; exit 1; \
	fi

# Unit tests (no credentials needed).
test:
	mvn -q test -Dtest='!**/*E2ETest'

# End-to-end tests against the module CDN and ingestion.
test-e2e:
	@test -n "$$CONFIDENCE_CLIENT_SECRET" -a -n "$$CONFIDENCE_CLIENT_ENCRYPTION_KEY" || \
		(echo "CONFIDENCE_CLIENT_SECRET and CONFIDENCE_CLIENT_ENCRYPTION_KEY must be set" && exit 1)
	mvn -q test -Dtest='**/*E2ETest'

lint:
	mvn -q fmt:check

clean:
	mvn -q clean

# Regenerate the state-to-wasm protobuf classes from ../proto.
proto:
	mvn -q protobuf:compile

.DEFAULT_GOAL := build
