package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.spotify.confidence.sdk.MaterializationStore.ReadOp;
import com.spotify.confidence.sdk.MaterializationStore.ReadResult;
import com.spotify.confidence.sdk.MaterializationStore.WriteOp;
import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.util.List;
import java.util.Set;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;

/**
 * The hosted store's hand-encoded protobuf wire, as the Go provider's remote_materialization_
 * store_test.go; that file is the reference for the bytes.
 */
class RemoteMaterializationStoreTest {

  private HttpServer server;
  private RemoteMaterializationStore store;

  // ReadOperationsRequest{ops: [ReadOp{inclusion_read_op{unit: "user-a", materialization: "m"}}]}
  private static final byte[] WANT_READ = {
    0x1a, 0x0d, 0x12, 0x0b, 0x0a, 0x06, 'u', 's', 'e', 'r', '-', 'a', 0x12, 0x01, 'm'
  };
  // ReadOperationsResult{results: [ReadResult{inclusion_result{unit, materialization, is_included:
  // true}}]}
  private static final byte[] READ_RESPONSE = {
    0x0a, 0x0f, 0x12, 0x0d, 0x0a, 0x06, 'u', 's', 'e', 'r', '-', 'a', 0x12, 0x01, 'm', 0x18, 0x01
  };
  // WriteOperationsRequest{store_variant_op: VariantData{unit "u", materialization "m",
  // rule "r", variant "v"}}
  private static final byte[] WANT_WRITE = {
    0x0a, 0x0c, 0x0a, 0x01, 'u', 0x12, 0x01, 'm', 0x1a, 0x01, 'r', 0x22, 0x01, 'v'
  };

  static final class Captured {
    volatile String path;
    volatile String authorization;
    volatile String contentType;
    volatile byte[] body;
  }

  private RemoteMaterializationStore started(Captured captured, byte[] response) {
    try {
      server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
      server.createContext(
          "/",
          exchange -> {
            captured.path = exchange.getRequestURI().getPath();
            captured.authorization = exchange.getRequestHeaders().getFirst("Authorization");
            captured.contentType = exchange.getRequestHeaders().getFirst("Content-Type");
            captured.body = exchange.getRequestBody().readAllBytes();
            exchange.getResponseHeaders().add("Content-Type", "application/x-protobuf");
            exchange.sendResponseHeaders(200, response == null ? -1 : response.length);
            if (response != null) {
              exchange.getResponseBody().write(response);
            }
            exchange.close();
          });
      server.start();
    } catch (IOException e) {
      throw new IllegalStateException(e);
    }
    store =
        new RemoteMaterializationStore("sec", "http://127.0.0.1:" + server.getAddress().getPort());
    return store;
  }

  @AfterEach
  void tearDown() {
    if (store != null) {
      store.shutdown();
    }
    if (server != null) {
      server.stop(0);
    }
  }

  @Test
  void testRemoteStoreWire() {
    Captured captured = new Captured();
    started(captured, READ_RESPONSE);

    List<ReadResult> results =
        store
            .read(List.of(new ReadOp.Inclusion("m", "user-a")))
            .toCompletableFuture()
            .orTimeout(5, TimeUnit.SECONDS)
            .join();

    assertEquals("/v1/materialization:readMaterializedOperations", captured.path);
    assertEquals("ClientSecret sec", captured.authorization);
    assertEquals("application/x-protobuf", captured.contentType);
    assertArrayEquals(WANT_READ, captured.body);

    assertEquals(1, results.size());
    ReadResult.Inclusion inclusion = (ReadResult.Inclusion) results.get(0);
    assertTrue(inclusion.included());
    assertEquals("user-a", inclusion.unit());
    assertEquals("m", inclusion.materialization());

    store
        .write(Set.of(new WriteOp.Variant("m", "u", "r", "v")))
        .toCompletableFuture()
        .orTimeout(5, TimeUnit.SECONDS)
        .join();

    assertEquals("/v1/materialization:writeMaterializedOperations", captured.path);
    assertArrayEquals(WANT_WRITE, captured.body);
  }

  @Test
  void testVariantReadRoundTrip() {
    // ReadOperationsResult{results: [ReadResult{variant_result{unit "u", materialization "m",
    // rule "r", variant "v"}}]}
    byte[] variantResponse = {
      0x0a, 0x0e, 0x0a, 0x0c, 0x0a, 0x01, 'u', 0x12, 0x01, 'm', 0x1a, 0x01, 'r', 0x22, 0x01, 'v'
    };
    Captured captured = new Captured();
    started(captured, variantResponse);

    List<ReadResult> results =
        store
            .read(List.of(new ReadOp.Variant("m", "u", "r")))
            .toCompletableFuture()
            .orTimeout(5, TimeUnit.SECONDS)
            .join();

    ReadResult.Variant variant = (ReadResult.Variant) results.get(0);
    assertEquals("v", variant.variant().orElse(null));
    assertEquals("r", variant.rule());
    assertEquals("u", variant.unit());

    // ReadOperationsRequest{ops: [ReadOp{variant_read_op{unit "u", materialization "m", rule
    // "r"}}]}
    byte[] wantRead = {0x1a, 0x0b, 0x0a, 0x09, 0x0a, 0x01, 'u', 0x12, 0x01, 'm', 0x1a, 0x01, 'r'};
    assertArrayEquals(wantRead, captured.body);
  }
}
