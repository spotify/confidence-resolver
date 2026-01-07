package com.spotify.confidence.sdk;

/**
 * Thrown when a {@link MaterializationStore} doesn't support the requested operation.
 *
 * <p>This exception triggers the provider to fall back to remote gRPC resolution via the Confidence
 * service, which manages materializations server-side.
 *
 * <p>Users typically don't need to catch this exception - it's handled internally by the provider's
 * resolution logic.
 *
 * @see UnsupportedMaterializationStore
 * @see MaterializationStore
 */
public class MaterializationNotSupportedException extends RuntimeException {}
