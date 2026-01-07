package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.*;

import io.grpc.ClientInterceptor;
import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.Test;

/** Tests to verify that the ChannelFactory pattern works correctly. */
public class ChannelFactoryTest {

  @Test
  public void verifyCustomChannelFactoryIsCalledByProvider() {
    final AtomicInteger factoryCallCount = new AtomicInteger(0);
    final List<String> targetsReceived = new ArrayList<>();
    final List<Integer> interceptorCounts = new ArrayList<>();

    final ChannelFactory customFactory =
        new ChannelFactory() {
          @Override
          public ManagedChannel create(String target, List<ClientInterceptor> interceptors) {
            factoryCallCount.incrementAndGet();
            targetsReceived.add(target);
            interceptorCounts.add(interceptors.size());
            ManagedChannelBuilder<?> builder =
                ManagedChannelBuilder.forTarget("localhost").usePlaintext();

            if (!interceptors.isEmpty()) {
              builder.intercept(interceptors.toArray(new ClientInterceptor[0]));
            }
            return builder.build();
          }
        };

    new OpenFeatureLocalResolveProvider(new LocalProviderConfig(customFactory), "clientsecret");

    assertEquals(
        1,
        factoryCallCount.get(),
        "ChannelFactory should have been called once for flag logger, but was called "
            + factoryCallCount.get()
            + " times");

    assertFalse(targetsReceived.isEmpty(), "Factory should have received target addresses");

    assertTrue(
        targetsReceived.get(0).contains("grpc") || targetsReceived.get(0).contains("edge"),
        "Target should be a gRPC endpoint, got: " + targetsReceived.get(0));
    assertEquals(1, interceptorCounts.size(), "Interceptors should have been called");
  }

  @Test
  public void verifyDefaultChannelFactoryIsUsedWhenNoneProvided() {
    final LocalProviderConfig config = new LocalProviderConfig();
    assertInstanceOf(
        DefaultChannelFactory.class,
        config.getChannelFactory(),
        "LocalProviderConfig should use DefaultChannelFactory when none is provided");
  }
}
