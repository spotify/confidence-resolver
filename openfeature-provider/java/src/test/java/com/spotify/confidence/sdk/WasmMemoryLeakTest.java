package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.assertEquals;

import com.dylibso.chicory.runtime.Instance;
import com.google.protobuf.Struct;
import com.google.protobuf.util.Structs;
import com.google.protobuf.util.Values;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import java.lang.reflect.Field;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Test;

/**
 * Regression test for WASM memory leak in host functions. Each resolve_flags call triggers a
 * current_time() host call which allocates a request in WASM memory. If the host doesn't free it,
 * memory leaks ~20 bytes per call and eventually forces memory.grow.
 */
class WasmMemoryLeakTest {

  private static int getWasmMemoryPages(WasmLocalResolver resolver) {
    try {
      Field instanceField = WasmLocalResolver.class.getDeclaredField("instance");
      instanceField.setAccessible(true);
      Instance instance = (Instance) instanceField.get(resolver);
      return instance.memory().pages();
    } catch (ReflectiveOperationException e) {
      throw new RuntimeException("Failed to access WASM memory via reflection", e);
    }
  }

  @Test
  void wasmMemoryStableOnRepeatedResolveCalls() {
    WasmLocalResolver resolver = new WasmLocalResolver(request -> {}, Map.of());
    resolver.setResolverState(ResolveTest.exampleStateBytes, "account", null);

    ResolveProcessRequest request =
        ResolveProcessRequest.newBuilder()
            .setDeferredMaterializations(
                ResolveFlagsRequest.newBuilder()
                    .addAllFlags(List.of("flags/flag-1"))
                    .setClientSecret(ResolveTest.secret.getSecret())
                    .setEvaluationContext(
                        Structs.of(
                            "targeting_key",
                            Values.of("user-123"),
                            "bar",
                            Values.of(Struct.newBuilder().build())))
                    .setApply(true)
                    .build())
            .build();

    // Warm up to settle one-time allocations
    for (int i = 0; i < 50_000; i++) {
      resolver.resolveProcess(request).toCompletableFuture().join();
      if (i % 1000 == 0) resolver.flushAllLogs();
    }

    int pagesBefore = getWasmMemoryPages(resolver);

    for (int i = 0; i < 50_000; i++) {
      resolver.resolveProcess(request).toCompletableFuture().join();
      if (i % 1000 == 0) resolver.flushAllLogs();
    }

    int pagesAfter = getWasmMemoryPages(resolver);

    assertEquals(
        pagesBefore,
        pagesAfter,
        String.format(
            "WASM memory grew from %d to %d pages (%d bytes leaked) — "
                + "host function is not freeing guest request allocations",
            pagesBefore, pagesAfter, (pagesAfter - pagesBefore) * 65536L));
  }
}
