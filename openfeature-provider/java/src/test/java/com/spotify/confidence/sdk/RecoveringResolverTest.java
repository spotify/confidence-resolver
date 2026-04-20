package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.*;

import com.dylibso.chicory.runtime.ChicoryInterruptedException;
import com.dylibso.chicory.wasm.ChicoryException;
import com.spotify.confidence.sdk.flags.resolver.v1.Sdk;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.Test;

class RecoveringResolverTest {

  private static final byte[] STATE = new byte[] {1, 2, 3};
  private static final String ACCOUNT_ID = "account";
  private static final Sdk SDK = Sdk.getDefaultInstance();

  @Test
  void chicoryExceptionTriggersRecreation() throws Exception {
    final LocalResolver failing = mock(LocalResolver.class);
    doThrow(new ChicoryException("wasm trap")).when(failing).setResolverState(any(), any(), any());

    final LocalResolver replacement = mock(LocalResolver.class);
    final AtomicInteger factoryCalls = new AtomicInteger();
    final RecoveringResolver recovering =
        new RecoveringResolver(
            () -> {
              int call = factoryCalls.incrementAndGet();
              return call == 1 ? failing : replacement;
            });

    assertThatThrownBy(() -> recovering.setResolverState(STATE, ACCOUNT_ID, SDK))
        .isInstanceOf(ChicoryException.class);

    Thread.sleep(200);
    assertThat(factoryCalls.get()).isEqualTo(2);
  }

  @Test
  void interruptedExceptionDoesNotTriggerRecreation() throws Exception {
    final LocalResolver failing = mock(LocalResolver.class);
    doThrow(new ChicoryInterruptedException("Thread interrupted"))
        .when(failing)
        .setResolverState(any(), any(), any());

    final AtomicInteger factoryCalls = new AtomicInteger();
    final RecoveringResolver recovering =
        new RecoveringResolver(
            () -> {
              factoryCalls.incrementAndGet();
              return failing;
            });

    assertThatThrownBy(() -> recovering.setResolverState(STATE, ACCOUNT_ID, SDK))
        .isInstanceOf(ChicoryInterruptedException.class);

    Thread.sleep(200);
    assertThat(factoryCalls.get()).isEqualTo(1);
  }
}
