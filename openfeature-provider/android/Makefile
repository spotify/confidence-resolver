.PHONY: build test clean

build:
	./gradlew build

test:
	./gradlew test

clean:
	./gradlew clean

assemble:
	./gradlew assemble

lint:
	./gradlew lint

publish-local:
	./gradlew publishToMavenLocal

copy-wasm:
	cp ../../wasm/confidence_resolver.wasm confidence-provider/src/main/resources/wasm/
