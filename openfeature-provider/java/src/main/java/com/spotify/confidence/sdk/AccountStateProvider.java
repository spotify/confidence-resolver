package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.admin.v1.LogDestination;
import java.util.Collections;
import java.util.List;

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

  /**
   * Returns the log destinations from the CDN state. First entry is primary, second is fallback.
   * Empty defaults to {@link LogDestination#LOG_DESTINATION_SPOTIFY_EDGE}.
   *
   * @return the list of log destinations
   */
  default List<LogDestination> logDestinations() {
    return Collections.emptyList();
  }

  void reload();
}
