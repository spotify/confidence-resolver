package com.spotify.confidence.sdk;

/**
 * Interface for providing AccountState instances.
 *
 * <p>The untyped nature of this interface allows high flexibility for testing, but it's not advised
 * to be used in production.
 *
 * <p>This can be useful if the provider implementer defines the AccountState proto schema in a
 * different Java package.
 */
public interface AccountStateProvider {

  /**
   * Provides an AccountState protobuf, from this proto specification: {@code
   * com.spotify.confidence.sdk.flags.admin.v1.AccountState}
   *
   * @return the AccountState protobuf containing flag configurations and metadata
   * @throws RuntimeException if the AccountState cannot be provided
   */
  byte[] provide();

  /**
   * Provides the account identifier associated with the account state.
   *
   * @return the account ID string
   */
  String accountId();

  void reload();
}
