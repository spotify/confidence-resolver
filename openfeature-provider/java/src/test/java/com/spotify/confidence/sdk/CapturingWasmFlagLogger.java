package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * A WasmFlagLogger implementation that captures all WriteFlagLogsRequest objects for testing.
 *
 * <p>This logger stores all requests in a thread-safe list, allowing tests to verify:
 *
 * <ul>
 *   <li>Flag names that were resolved
 *   <li>Targeting keys used for evaluation
 *   <li>Assignment IDs generated
 *   <li>Variant information
 *   <li>Client and credential information
 * </ul>
 *
 * <p>Usage example:
 *
 * <pre>{@code
 * CapturingWasmFlagLogger capturingLogger = new CapturingWasmFlagLogger();
 * // ... create provider with capturingLogger ...
 * // ... perform flag evaluations ...
 *
 * List<WriteFlagLogsRequest> captured = capturingLogger.getCapturedRequests();
 * assertThat(captured).hasSize(1);
 * assertThat(captured.get(0).getFlagAssignedList()).hasSize(1);
 * }</pre>
 */
public class CapturingWasmFlagLogger implements WasmFlagLogger {
  private final List<WriteFlagLogsRequest> capturedRequests = new CopyOnWriteArrayList<>();
  private volatile boolean shutdownCalled = false;

  @Override
  public void write(WriteFlagLogsRequest request) {
    capturedRequests.add(request);
  }

  @Override
  public void writeSync(WriteFlagLogsRequest request) {
    capturedRequests.add(request);
  }

  @Override
  public void shutdown() {
    shutdownCalled = true;
  }

  /**
   * Returns an unmodifiable view of all captured WriteFlagLogsRequest objects.
   *
   * @return list of all captured requests
   */
  public List<WriteFlagLogsRequest> getCapturedRequests() {
    return Collections.unmodifiableList(new ArrayList<>(capturedRequests));
  }

  /** Clears all captured requests. */
  public void clear() {
    capturedRequests.clear();
  }

  /**
   * Returns the total number of captured requests.
   *
   * @return number of captured requests
   */
  public int getCapturedCount() {
    return capturedRequests.size();
  }

  /**
   * Returns whether shutdown() was called on this logger.
   *
   * @return true if shutdown was called
   */
  public boolean wasShutdownCalled() {
    return shutdownCalled;
  }

  /**
   * Returns the total number of FlagAssigned entries across all captured requests.
   *
   * @return total flag assigned count
   */
  public int getTotalFlagAssignedCount() {
    return capturedRequests.stream().mapToInt(WriteFlagLogsRequest::getFlagAssignedCount).sum();
  }

  /**
   * Returns the total number of ClientResolveInfo entries across all captured requests.
   *
   * @return total client resolve info count
   */
  public int getTotalClientResolveInfoCount() {
    return capturedRequests.stream()
        .mapToInt(WriteFlagLogsRequest::getClientResolveInfoCount)
        .sum();
  }

  /**
   * Returns the total number of FlagResolveInfo entries across all captured requests.
   *
   * @return total flag resolve info count
   */
  public int getTotalFlagResolveInfoCount() {
    return capturedRequests.stream().mapToInt(WriteFlagLogsRequest::getFlagResolveInfoCount).sum();
  }
}
