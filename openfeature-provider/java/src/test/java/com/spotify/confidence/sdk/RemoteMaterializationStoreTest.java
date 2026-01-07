package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.*;

import com.spotify.confidence.flags.resolver.v1.*;
import io.grpc.Status;
import io.grpc.StatusRuntimeException;
import java.time.Duration;
import java.util.*;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.Executors;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;

class RemoteMaterializationStoreTest {

  private InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub mockStub;
  private RemoteMaterializationStore store;

  @BeforeEach
  void setUp() {
    mockStub = mock(InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub.class);
    when(mockStub.withDeadlineAfter(anyLong(), any())).thenReturn(mockStub);
    store =
        new RemoteMaterializationStore(
            mockStub,
            Duration.ofSeconds(2),
            Duration.ofSeconds(5),
            Executors.newCachedThreadPool());
  }

  @Test
  void testReadEmptyOps() throws Exception {
    List<MaterializationStore.ReadResult> results =
        store.read(Collections.emptyList()).toCompletableFuture().get();
    assertEquals(0, results.size());
    verify(mockStub, never()).readMaterializedOperations(any());
  }

  @Test
  void testReadVariantOp() throws Exception {
    ReadOperationsResult mockResponse =
        ReadOperationsResult.newBuilder()
            .addResults(
                com.spotify.confidence.flags.resolver.v1.ReadResult.newBuilder()
                    .setVariantResult(
                        VariantData.newBuilder()
                            .setMaterialization("exp_v1")
                            .setUnit("user-123")
                            .setRule("rule-1")
                            .setVariant("variant-a")
                            .build())
                    .build())
            .build();

    when(mockStub.readMaterializedOperations(any())).thenReturn(mockResponse);

    List<MaterializationStore.ReadOp> ops =
        List.of(new MaterializationStore.ReadOp.Variant("exp_v1", "user-123", "rule-1"));

    List<MaterializationStore.ReadResult> results = store.read(ops).toCompletableFuture().get();

    assertEquals(1, results.size());
    assertTrue(results.get(0) instanceof MaterializationStore.ReadResult.Variant);

    MaterializationStore.ReadResult.Variant variantResult =
        (MaterializationStore.ReadResult.Variant) results.get(0);
    assertEquals("exp_v1", variantResult.materialization());
    assertEquals("user-123", variantResult.unit());
    assertEquals("rule-1", variantResult.rule());
    assertTrue(variantResult.variant().isPresent());
    assertEquals("variant-a", variantResult.variant().get());
  }

  @Test
  void testReadInclusionOp() throws Exception {
    ReadOperationsResult mockResponse =
        ReadOperationsResult.newBuilder()
            .addResults(
                com.spotify.confidence.flags.resolver.v1.ReadResult.newBuilder()
                    .setInclusionResult(
                        InclusionData.newBuilder()
                            .setMaterialization("segment_v1")
                            .setUnit("user-456")
                            .setIsIncluded(true)
                            .build())
                    .build())
            .build();

    when(mockStub.readMaterializedOperations(any())).thenReturn(mockResponse);

    List<MaterializationStore.ReadOp> ops =
        List.of(new MaterializationStore.ReadOp.Inclusion("segment_v1", "user-456"));

    List<MaterializationStore.ReadResult> results = store.read(ops).toCompletableFuture().get();

    assertEquals(1, results.size());
    assertTrue(results.get(0) instanceof MaterializationStore.ReadResult.Inclusion);

    MaterializationStore.ReadResult.Inclusion inclusionResult =
        (MaterializationStore.ReadResult.Inclusion) results.get(0);
    assertEquals("segment_v1", inclusionResult.materialization());
    assertEquals("user-456", inclusionResult.unit());
    assertTrue(inclusionResult.included());
  }

  @Test
  void testReadDeadlineExceeded() {
    when(mockStub.readMaterializedOperations(any()))
        .thenThrow(new StatusRuntimeException(Status.DEADLINE_EXCEEDED));

    List<MaterializationStore.ReadOp> ops =
        List.of(new MaterializationStore.ReadOp.Variant("exp_v1", "user-123", "rule-1"));

    ExecutionException exception =
        assertThrows(ExecutionException.class, () -> store.read(ops).toCompletableFuture().get());

    assertTrue(exception.getCause() instanceof RuntimeException);
    assertTrue(
        exception.getCause().getMessage().contains("Failed to read materialized operations"));
  }

  @Test
  void testReadUnauthenticated() {
    when(mockStub.readMaterializedOperations(any()))
        .thenThrow(new StatusRuntimeException(Status.UNAUTHENTICATED));

    List<MaterializationStore.ReadOp> ops =
        List.of(new MaterializationStore.ReadOp.Variant("exp_v1", "user-123", "rule-1"));

    ExecutionException exception =
        assertThrows(ExecutionException.class, () -> store.read(ops).toCompletableFuture().get());

    assertTrue(exception.getCause() instanceof RuntimeException);
    assertTrue(
        exception.getCause().getMessage().contains("Failed to read materialized operations"));
  }

  @Test
  void testReadUnavailable() {
    when(mockStub.readMaterializedOperations(any()))
        .thenThrow(new StatusRuntimeException(Status.UNAVAILABLE));

    List<MaterializationStore.ReadOp> ops =
        List.of(new MaterializationStore.ReadOp.Variant("exp_v1", "user-123", "rule-1"));

    ExecutionException exception =
        assertThrows(ExecutionException.class, () -> store.read(ops).toCompletableFuture().get());

    assertTrue(exception.getCause() instanceof RuntimeException);
    assertTrue(
        exception.getCause().getMessage().contains("Failed to read materialized operations"));
  }

  @Test
  void testWriteEmptyOps() throws Exception {
    store.write(Collections.emptySet()).toCompletableFuture().get();
    verify(mockStub, never()).writeMaterializedOperations(any());
  }

  @Test
  void testWriteVariantOp() throws Exception {
    WriteOperationsResult mockResponse = WriteOperationsResult.newBuilder().build();
    when(mockStub.writeMaterializedOperations(any())).thenReturn(mockResponse);

    Set<MaterializationStore.WriteOp> ops =
        Set.of(
            new MaterializationStore.WriteOp.Variant("exp_v1", "user-123", "rule-1", "variant-a"));

    store.write(ops).toCompletableFuture().get();

    ArgumentCaptor<WriteOperationsRequest> requestCaptor =
        ArgumentCaptor.forClass(WriteOperationsRequest.class);
    verify(mockStub).writeMaterializedOperations(requestCaptor.capture());

    WriteOperationsRequest request = requestCaptor.getValue();
    assertEquals(1, request.getStoreVariantOpCount());
    VariantData variantData = request.getStoreVariantOp(0);
    assertEquals("exp_v1", variantData.getMaterialization());
    assertEquals("user-123", variantData.getUnit());
    assertEquals("rule-1", variantData.getRule());
    assertEquals("variant-a", variantData.getVariant());
  }

  @Test
  void testWriteDeadlineExceeded() {
    when(mockStub.writeMaterializedOperations(any()))
        .thenThrow(new StatusRuntimeException(Status.DEADLINE_EXCEEDED));

    Set<MaterializationStore.WriteOp> ops =
        Set.of(
            new MaterializationStore.WriteOp.Variant("exp_v1", "user-123", "rule-1", "variant-a"));

    ExecutionException exception =
        assertThrows(ExecutionException.class, () -> store.write(ops).toCompletableFuture().get());

    assertTrue(exception.getCause() instanceof RuntimeException);
    assertTrue(
        exception.getCause().getMessage().contains("Failed to write materialized operations"));
  }

  @Test
  void testWritePermissionDenied() {
    when(mockStub.writeMaterializedOperations(any()))
        .thenThrow(new StatusRuntimeException(Status.PERMISSION_DENIED));

    Set<MaterializationStore.WriteOp> ops =
        Set.of(
            new MaterializationStore.WriteOp.Variant("exp_v1", "user-123", "rule-1", "variant-a"));

    ExecutionException exception =
        assertThrows(ExecutionException.class, () -> store.write(ops).toCompletableFuture().get());

    assertTrue(exception.getCause() instanceof RuntimeException);
    assertTrue(
        exception.getCause().getMessage().contains("Failed to write materialized operations"));
  }

  @Test
  void testReadMultipleOps() throws Exception {
    ReadOperationsResult mockResponse =
        ReadOperationsResult.newBuilder()
            .addResults(
                com.spotify.confidence.flags.resolver.v1.ReadResult.newBuilder()
                    .setVariantResult(
                        VariantData.newBuilder()
                            .setMaterialization("exp_v1")
                            .setUnit("user-123")
                            .setRule("rule-1")
                            .setVariant("variant-a")
                            .build())
                    .build())
            .addResults(
                com.spotify.confidence.flags.resolver.v1.ReadResult.newBuilder()
                    .setInclusionResult(
                        InclusionData.newBuilder()
                            .setMaterialization("segment_v1")
                            .setUnit("user-456")
                            .setIsIncluded(false)
                            .build())
                    .build())
            .build();

    when(mockStub.readMaterializedOperations(any())).thenReturn(mockResponse);

    List<MaterializationStore.ReadOp> ops =
        List.of(
            new MaterializationStore.ReadOp.Variant("exp_v1", "user-123", "rule-1"),
            new MaterializationStore.ReadOp.Inclusion("segment_v1", "user-456"));

    List<MaterializationStore.ReadResult> results = store.read(ops).toCompletableFuture().get();

    assertEquals(2, results.size());
  }

  @Test
  void testWriteMultipleOps() throws Exception {
    WriteOperationsResult mockResponse = WriteOperationsResult.newBuilder().build();
    when(mockStub.writeMaterializedOperations(any())).thenReturn(mockResponse);

    Set<MaterializationStore.WriteOp> ops =
        Set.of(
            new MaterializationStore.WriteOp.Variant("exp_v1", "user-123", "rule-1", "variant-a"),
            new MaterializationStore.WriteOp.Variant("exp_v2", "user-456", "rule-2", "variant-b"));

    store.write(ops).toCompletableFuture().get();

    ArgumentCaptor<WriteOperationsRequest> requestCaptor =
        ArgumentCaptor.forClass(WriteOperationsRequest.class);
    verify(mockStub).writeMaterializedOperations(requestCaptor.capture());

    WriteOperationsRequest request = requestCaptor.getValue();
    assertEquals(2, request.getStoreVariantOpCount());
  }

  @Test
  void testReadVariantWithEmptyOptional() throws Exception {
    ReadOperationsResult mockResponse =
        ReadOperationsResult.newBuilder()
            .addResults(
                com.spotify.confidence.flags.resolver.v1.ReadResult.newBuilder()
                    .setVariantResult(
                        VariantData.newBuilder()
                            .setMaterialization("exp_v1")
                            .setUnit("user-123")
                            .setRule("rule-1")
                            // No variant set, meaning empty optional
                            .build())
                    .build())
            .build();

    when(mockStub.readMaterializedOperations(any())).thenReturn(mockResponse);

    List<MaterializationStore.ReadOp> ops =
        List.of(new MaterializationStore.ReadOp.Variant("exp_v1", "user-123", "rule-1"));

    List<MaterializationStore.ReadResult> results = store.read(ops).toCompletableFuture().get();

    assertEquals(1, results.size());
    MaterializationStore.ReadResult.Variant variantResult =
        (MaterializationStore.ReadResult.Variant) results.get(0);
    assertFalse(variantResult.variant().isPresent());
  }
}
