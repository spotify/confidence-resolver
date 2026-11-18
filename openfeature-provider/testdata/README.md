# Shared provider test data

Resolver states, and the state-to-wasm modules compiled from them, shared by every provider's tests.

- `states/*.json`: purpose-built `ResolverState`s (proto3 JSON).
- `modules/*.wasm`: modules compiled from the state of the same name. They are committed, so the tests
  need no compiler.

| State | Covers |
|-------|--------|
| `basic` | `flags/plain`: a struct schema (bool, string, int, double, nested struct), matching all units. `flags/sticky`: a paused-intake rule reading `materializedSegments/sticky`, plus a fallback rule. Client secret `test-secret`. |

## How the modules are built

The modules are compiled with the state-to-wasm compiler (not yet public) using these options:

- `--threads`: the shared-memory (threaded) variant
- `-E`: export `evaluate`

With [JBang](https://www.jbang.dev/) and access to the compiler artifact, from the repository root
(`1.0.0-SNAPSHOT` after `mvn install` in a state-to-wasm checkout, or a release):

```bash
jbang com.spotify.confidence:state-to-wasm-compiler:<version> \
  openfeature-provider/testdata/states/basic.json --threads -E \
  -o openfeature-provider/testdata/modules/basic.wasm
```

Rebuild a module whenever its state changes, and commit both files together. Record the compiler
revision here. (A module's `confidence.metadata` section carries the compiler Git SHA only for builds
from a clean checkout.)

Current modules: state-to-wasm `107bbc98`.
