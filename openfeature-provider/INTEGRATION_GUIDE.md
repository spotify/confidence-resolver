# Confidence OpenFeature Provider Integration Guide

This guide contains common integration steps that apply to all Confidence OpenFeature providers in this repository.

For language-specific installation and quick start instructions, see your provider's README:
- [Go Provider](go/README.md)
- [Java Provider](java/README.md)
- [JavaScript Provider](js/README.md)
- [Ruby Provider](ruby/README.md)

---

## Table of Contents

1. [Getting Your Credentials](#getting-your-credentials)
2. [Error Handling](#error-handling)
3. [Sticky Assignments](#sticky-assignments)
4. [Deferred Apply and Resolve Token Security](#deferred-apply-and-resolve-token-security)

---

## Getting Your Credentials

Before integrating any Confidence provider, you'll need a **client secret** from your Confidence account:

1. Log into the Confidence dashboard
2. In the **Clients** section, create a new client secret for the client you intend to use (or start by creating a new client)
3. Make sure to select **Backend** as integration type. Never expose your Backend client secret outside your organization

---

## Error Handling

All Confidence providers use a **default value fallback** pattern to ensure your application continues to function even when flag evaluation fails.

### How Default Values Work

When you request a flag value, you always provide a default:

```
// Pseudocode
value = client.getFlagValue("my-flag", DEFAULT_VALUE, context)
```

If anything goes wrong, the provider returns `DEFAULT_VALUE` instead of throwing an error.

### Common Failure Scenarios

| Scenario | What Happens | Common Causes |
|----------|--------------|---------------|
| **Flag doesn't exist** | Returns default | Flag not created, wrong name, not enabled for the client |
| **Type mismatch** | Returns default | Requesting boolean for string or object property. Or requesting boolean for the _flag_. Flags are objects in Confidence |
| **Network failure** | Returns default | Confidence API unreachable (Ruby only) |
| **Initialization failure** | Returns default | CDN unreachable, invalid credentials not backend type |
| **Invalid context** | Returns default | Malformed attributes, missing targeting key |
| **Provider not ready** | Returns default | Called before initialization complete |

### Error Details

For debugging, use the `details` methods to get error information:

**Error codes:**
- `FLAG_NOT_FOUND`: The flag doesn't exist in Confidence
- `TYPE_MISMATCH`: Wrong value type requested (e.g., boolean for string)
- `PROVIDER_NOT_READY`: Provider still initializing
- `PARSE_ERROR`: Response couldn't be parsed
- `GENERAL_ERROR`: Other errors (network, timeout, etc.)

**Reasons** (standard OpenFeature reasons):
- `TARGETING_MATCH`: Flag evaluated successfully and matched targeting rules
- `DEFAULT`: Default value returned (no segment/variant matched)
- `DISABLED`: Flag is disabled or archived
- `STALE`: Stale cached value
- `ERROR`: Evaluation failed (see error code)
- `UNKNOWN`: Reason could not be determined

### Production Best Practices

1. **Choose safe defaults**
   Example:
   ```
   ✅ GOOD: Default to "off" for risky features
   ❌ BAD: Default to "on" for untested code
   ```

2. **Log errors for debugging**
   - Track evaluation failures in your monitoring system. You can use OpenFeature [hooks](https://openfeature.dev/docs/reference/concepts/hooks/) for this.
   - Include flag key, error code, and context in logs
   - Set up alerts for elevated error rates

3. **Monitor error rates**
   - Track `errorCode != null` metrics
   - Alert if error rate exceeds threshold (e.g., >5%)
   - Investigate spikes (may indicate misconfigured flag setup or SDK integration)

4. **Test error scenarios**
   - Verify app works when Confidence is unreachable
   - Test with invalid credentials
   - Test with non-existent flags
   - Verify graceful handling of type mismatches

5. **Document your defaults**
   ```
   // Default: false - feature is opt-in for safety
   const enabled = getFlag("new-payment-flow", false)

   // Default: 1000ms - conservative timeout
   const timeout = getFlag("api-timeout", 1000)
   ```

---

## Sticky Assignments

Confidence provides **sticky** flag assignments to ensure users receive consistent variant assignments across evaluations. It can be used for two things:
- Pause intake of new entities to an experiment
- Ensure that entities are assigned the same variant throughout an experiment even if some of their targeting attributes change during the experiment.

### What are Sticky Assignments?

When a flag is evaluated for a user, Confidence creates a **materialization** — a snapshot of which variant that user was assigned. On subsequent evaluations, the same variant is returned even if:

- The user's context attributes change (e.g., different country, device type)
- The flag's targeting rules are modified
- New assignments are paused (controlled rollouts)

### How It Works

By default, **sticky assignments are managed by Confidence servers**:

1. First, the local WASM resolver attempts to resolve the flag
2. If sticky assignment data is needed, the provider makes a network call to Confidence's cloud resolvers
3. Materializations are stored on Confidence servers with a **90-day TTL** (automatically renewed on access)
4. No local storage or database setup required

### Benefits

- **Zero configuration**: Works out of the box with no additional setup
- **Managed storage**: Confidence handles all storage and consistency
- **Global availability**: Materializations are available across all your services that are using this flag

### Latency Considerations

When a sticky assignment is needed, the provider makes a network call to Confidence's cloud resolvers. This introduces additional latency (the network latency between your location and Confidence servers) compared to local WASM evaluation.

### Custom Materialization Storage

Some providers support custom storage backends to eliminate network calls for sticky assignments. Check your provider's README for availability and implementation details:

- [Java Provider](java/README.md#sticky-assignments) - Supports custom `MaterializationRepository`
- [JavaScript Provider](js/README.md#sticky-assignments) - Coming soon
- [Go Provider](go/README.md) - Coming soon

### Deep Dive

For technical details on how sticky assignments work at the protocol level, including flowcharts, behavior matrices, and configuration patterns, see the [Sticky Assignments Technical Guide](../STICKY_ASSIGNMENTS.md).

---

## Deferred Apply and Resolve Token Security

Currently supported in the **Go**, **JavaScript**, and **Java** providers.

When you resolve flags with `apply=false`, the response includes a **resolve token** that you later pass to the apply call to record exposure. This pattern is useful when resolution happens earlier than exposure — for example, resolving on the server but only logging exposure once the client actually renders the experience (see the JS [React integration](js/README-REACT.md) for a typical RSC flow).

### What the resolve token contains

The resolve token is **not encrypted**. It is a serialized payload that includes, among other things:

- The full evaluation context used during resolution (targeting key and any attributes you passed in)
- Which variant each flag was resolved to

### Recommendation

If any of that data is sensitive (PII in the evaluation context, variant assignments you don't want exposed to end users), do not let the raw token leave your backend. Encrypt the token at the trust boundary and decrypt it again before handing it back to the provider's apply call:

```
backend ──[ resolve(apply=false) ]──► resolve token (plaintext)
backend ──[ encrypt ]──► opaque token ──► client / storage / queue
client ──► opaque token ──► backend ──[ decrypt ]──► resolve token ──► provider.applyFlag(...)
```

The provider only needs to see the original token at apply time — anything you wrap around it in transit is up to you.

---

## Additional Resources

- [Confidence Documentation](https://confidence.spotify.com/docs)
- [OpenFeature Specification](https://openfeature.dev/specification)
- [Provider-Specific READMEs](.)
  - [Go Provider](go/README.md)
  - [Java Provider](java/README.md)
  - [JavaScript Provider](js/README.md)
  - [Ruby Provider](ruby/README.md)
- [Root Repository README](../README.md)
- [Sticky Assignments Technical Guide](../STICKY_ASSIGNMENTS.md)

