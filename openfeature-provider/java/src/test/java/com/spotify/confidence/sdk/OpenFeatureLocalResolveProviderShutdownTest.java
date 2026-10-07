package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatCode;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.CALLS_REAL_METHODS;
import static org.mockito.Mockito.clearInvocations;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.verifyNoInteractions;

import ch.qos.logback.classic.Level;
import ch.qos.logback.classic.Logger;
import ch.qos.logback.classic.spi.ILoggingEvent;
import ch.qos.logback.core.read.ListAppender;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import dev.openfeature.sdk.ImmutableContext;
import java.io.File;
import java.nio.file.Files;
import java.util.concurrent.RejectedExecutionException;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.slf4j.LoggerFactory;

class OpenFeatureLocalResolveProviderShutdownTest {

  private static final String CLIENT_SECRET = "shutdown-test-secret";
  private static final String ACCOUNT_NAME = "accounts/test-account";

  @BeforeAll
  static void beforeAll() {
    System.setProperty("CONFIDENCE_NUMBER_OF_WASM_INSTANCES", "1");
  }

  @Test
  void closeTwiceDoesNotFlushAgain() {
    final WasmFlagLogger flagLogger = mock(WasmFlagLogger.class);
    final WasmLocalResolver resolver = new WasmLocalResolver(flagLogger::write);
    resolver.close();
    verify(flagLogger).write(any(WriteFlagLogsRequest.class));
    clearInvocations(flagLogger);

    assertThatCode(resolver::close).doesNotThrowAnyException();
    verifyNoInteractions(flagLogger);
  }

  @Test
  void shutdownTwiceDoesNotThrowOrWarn() throws Exception {
    final byte[] stateBytes =
        Files.readAllBytes(
            new File(getClass().getResource("/resolver_state_current.pb").getPath()).toPath());
    final WasmFlagLogger flagLogger = mock(NoOpWasmFlagLogger.class, CALLS_REAL_METHODS);
    doAnswer(
            invocation -> {
              doThrow(new RejectedExecutionException("logger already shut down"))
                  .when(flagLogger)
                  .write(any());
              return null;
            })
        .when(flagLogger)
        .shutdown();
    final OpenFeatureLocalResolveProvider provider =
        new OpenFeatureLocalResolveProvider(
            new TestAccountStateProvider(stateBytes, ACCOUNT_NAME),
            CLIENT_SECRET,
            new UnsupportedMaterializationStore(),
            flagLogger);
    provider.initialize(new ImmutableContext());

    final Logger logger = (Logger) LoggerFactory.getLogger(RecoveringResolver.class);
    final ListAppender<ILoggingEvent> appender = new ListAppender<>();
    appender.start();
    logger.addAppender(appender);
    try {
      assertThatCode(
              () -> {
                provider.shutdown();
                provider.shutdown();
              })
          .doesNotThrowAnyException();
      assertThat(appender.list).noneMatch(event -> event.getLevel().isGreaterOrEqual(Level.WARN));
      verify(flagLogger).shutdown();
    } finally {
      logger.detachAppender(appender);
      appender.stop();
      provider.shutdown();
    }
  }

  @Test
  void shutdownDoesNotWaitForTheNextScheduledPoll() throws Exception {
    final byte[] stateBytes =
        Files.readAllBytes(
            new File(getClass().getResource("/resolver_state_current.pb").getPath()).toPath());

    final OpenFeatureLocalResolveProvider provider =
        new OpenFeatureLocalResolveProvider(
            new TestAccountStateProvider(stateBytes, ACCOUNT_NAME),
            CLIENT_SECRET,
            new UnsupportedMaterializationStore(),
            new NoOpWasmFlagLogger());

    provider.initialize(new ImmutableContext());
    provider.shutdown();

    assertThat(provider.forcedFetcherShutdown)
        .as(
            "shutdown should drop the pending poll and terminate without force-killing the executor")
        .isFalse();
  }
}
