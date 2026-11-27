# Migrating to the `next` providers

> [!IMPORTANT]
> This guide covers the release candidates built from this branch — the state2wasm line, where
> each credential's flags are compiled into a flag module fetched from the Confidence CDN.
> Rewritten so far: Java (`0.24.0-rc`, from `0.23`), Go (`0.30.0-rc`, from `0.29`). The
> JavaScript, Python, Ruby and Rust providers are not rewritten yet; keep using their `main`
> releases.

See the [repository README](../README.md) for what the rewrite changes and why, and the
[Integration Guide](INTEGRATION_GUIDE.md) for concepts shared with the `main` providers.

## Startup: `setProviderAndWait` throws when initialization fails

**The change most likely to break a Java service.** In Java `0.23`, a failed initial state load
only logged a warning: `initialize()` returned, the provider served flag defaults, and
`setProviderAndWait` returned normally — the service booted even while Confidence was
unreachable.

In `0.24.0-rc`, `initialize()` throws a `GeneralError` when initialization fails and
`setProviderAndWait` rethrows, so a service that registers the provider during startup — for
example in its flag-client factory — fails to boot during a flag CDN outage unless it handles
the error:

- **Timeout** — no flag module within `initializationTimeout`, 30 seconds by default. The CDN
  is unreachable, or the credential's module is still building. The provider keeps retrying in
  the background and becomes ready when the module lands.
- **Credential rejected** — the CDN rejected the client secret, or the module failed to decrypt
  with the encryption key. `FATAL`: no retry; fix the credential and restart.

```java
try {
    OpenFeatureAPI.getInstance().setProviderAndWait(provider);
} catch (OpenFeatureError e) {
    logger.error("Confidence provider init failed; serving flag defaults", e);
    // OpenFeatureAPI.getInstance().shutdown(); // stay on defaults for the whole process
}
```

Go services are not newly affected: `SetProviderAndWait` has returned this error since `0.27.1`,
with the same background recovery on a timeout.

```go
if err := openfeature.SetProviderAndWait(provider); err != nil {
    log.Printf("Confidence provider init failed; serving flag defaults: %v", err)
    // openfeature.Shutdown() // stay on defaults for the whole process
}
```

Flag *evaluations* are unchanged in both languages: they return your defaults while no module
is installed (`PROVIDER_NOT_READY`); only the registration call throws. Catching the failure
leaves the provider registered and recovering; decide per service whether to serve defaults,
stay on defaults for the whole process (the commented shutdown), or fail fast by letting the
error propagate. Details and tradeoffs: [Java](java/README.md#initialization-at-startup),
[Go](go/README.md#initialization-at-startup).

## Other breaking changes

- **Encryption is mandatory.** Constructors require the credential's 64-hex-character
  encryption key; keyless constructors are gone, and missing or malformed keys fail before any
  network activity. See [Encryption](INTEGRATION_GUIDE.md#encryption).
- **Flags ship as compiled flag modules** from the module CDN
  (`confidence-resolver-module-cdn.spotifycdn.com`) instead of full-account resolver state.
- **`track()` is not supported yet** in the rewritten providers; it returns through the flag
  modules later.

For the full picture see the provider READMEs: [Java](java/README.md), [Go](go/README.md).
