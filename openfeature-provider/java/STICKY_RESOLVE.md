# Materialization Store Documentation

## Overview

The Materialization Store provides persistent storage for flag resolution data, supporting two key use cases:

1. **Sticky Assignments** - Maintain consistent variant assignments across evaluations even when targeting attributes change. This enables pausing intake (stopping new users from entering an experiment) while keeping existing users in their assigned variants.

2. **Custom Targeting via Materialized Segments** - Precomputed sets of identifiers from datasets that should be targeted. Instead of evaluating complex targeting rules at runtime, materializations allow efficient lookup of whether a unit (user, session, etc.) is included in a target segment.

**Default behavior:** no store is configured — flags that need materializations fall back to the
caller's default value. To persist sticky assignments and materialized segments, enable the
[hosted store](#how-it-works) (`useRemoteMaterializationStore(true)`) or provide your own; stored
materializations are managed by Confidence with an automatic 90-day TTL (renewed on access).

## How It Works

### Default: No Materialization Support (UnsupportedMaterializationStore)

**Flow:**
1. The flag module resolves locally and requests the materialization data it needs
2. The default `UnsupportedMaterializationStore` reports "not supported"
3. The evaluation falls back to the caller's default value (with a `GENERAL` error code);
   materialized segments still resolve through the membership data compiled into the module

**To get server-side sticky assignments and segments**, configure either the
[hosted store](#remote-materialization-store) (`useRemoteMaterializationStore(true)`) or your
own. Confidence manages stored materializations server-side with a 90-day TTL (auto-renewed on
access), and the Confidence UI can skip targeting criteria for sticky assignments and
pause/resume new entity intake.

### Hosted storage: RemoteMaterializationStore

`useRemoteMaterializationStore(true)` points the store at Confidence's hosted materialization
service: protobuf over HTTPS to the resolver gateway, authenticated with the client secret
(reads 2s, writes 5s; overridable via `CONFIDENCE_MATERIALIZATION_READ_TIMEOUT_SECONDS` and
`CONFIDENCE_MATERIALIZATION_WRITE_TIMEOUT_SECONDS`). See the README's
[Materialization Stores](README.md#materialization-stores).

### Custom: Local Storage (MaterializationStore)

Implement `MaterializationStore` to store materialization data locally and eliminate network calls.

**Interface:**
```java
public interface MaterializationStore {
  // Batch read of materialization data
  CompletionStage<List<ReadResult>> read(List<? extends ReadOp> ops);

  // Batch write of materialization data (optional)
  default CompletionStage<Void> write(Set<? extends WriteOp> ops) {
    throw new UnsupportedOperationException("Unimplemented method 'write'");
  }
}
```

**Key Concepts:**
- **Materialization** - Identifier for a materialization context (experiment, flag, or materialized segment)
- **Unit** - Entity identifier (user ID, session ID, etc.)
- **Rule** - Targeting rule identifier within a flag
- **Variant** - Assigned variant name for the unit+rule combination

**Operation Types:**

Read operations (`ReadOp`):
```java
// Check if unit is in materialized segment
sealed interface ReadOp {
  record Inclusion(String materialization, String unit) implements ReadOp {}
  record Variant(String materialization, String unit, String rule) implements ReadOp {}
}
```

Read results (`ReadResult`):
```java
sealed interface ReadResult {
  // Result for segment membership check
  record Inclusion(String materialization, String unit, boolean included) implements ReadResult {}

  // Result for sticky variant assignment
  record Variant(String materialization, String unit, String rule, Optional<String> variant)
      implements ReadResult {}
}
```

Write operations (`WriteOp`):
```java
sealed interface WriteOp {
  // Store sticky variant assignment
  record Variant(String materialization, String unit, String rule, String variant)
      implements WriteOp {}
}
```

## Implementation Examples

### In-Memory (Testing/Development Only)

**Warning: Do not use in-memory storage in production.** In-memory implementations lose all materialization data on restart, breaking sticky assignments and materialized segments. Production systems require persistent storage (Redis, database, etc.).

See `InMemoryMaterializationStoreExample` for a reference implementation. Use this pattern with persistent storage backends for production.

#### Usage

```java
MaterializationStore store = new InMemoryMaterializationStore();

OpenFeatureLocalResolveProvider provider = new OpenFeatureLocalResolveProvider(
    clientSecret,
    store
);
```

## Best Practices

1. **Thread-safe implementation** - Implementations must be thread-safe for concurrent flag resolution
2. **Fail gracefully** - Storage errors shouldn't fail flag resolution; throw `MaterializationNotSupportedException` and the evaluation falls back to the caller's default value
3. **Use 90-day TTL** - Match Confidence's default behavior, renew on read
4. **Idempotent writes** - Write operations should be idempotent
5. **Batch operations** - Both `read` and `write` accept batches for efficiency
6. **Connection pooling** - Use pools for Redis/DB connections
7. **Monitor metrics** - Track cache hit rate, storage latency, errors
8. **Test both paths** - Missing assignments (cold start) and existing assignments

## When to Use Custom Storage

| Implementation | Best For | Trade-offs |
|----------|----------|------------|
| No store (default) | Apps without sticky assignments | Zero network calls; flags needing materializations fall back to defaults. |
| **RemoteMaterializationStore** | Apps needing sticky assignments | Network calls during evaluation; managed by Confidence. |
| **MaterializationStore** (in-memory) | Testing/development only | Fast, no network. **Lost on restart - do not use in production.** |
| **MaterializationStore** (Redis/DB) | Production apps needing local storage | No network calls. Requires persistent storage infrastructure. |

**Start with the default.** Only implement custom storage with persistent backends (Redis, database) if you need to eliminate network calls or work offline.

## Error Handling

When implementing `read()`, throw `MaterializationNotSupportedException` for a graceful
fallback: the evaluation returns the caller's default value with a `GENERAL` error code instead
of failing. The engine joins reads (the store's `CompletionStage`), and writes are
fire-and-forget — a slow or failing store never fails or delays a resolve.

## Additional Resources

- [Confidence Sticky Assignments Documentation](https://confidence.spotify.com/docs/flags/audience#sticky-assignments)
