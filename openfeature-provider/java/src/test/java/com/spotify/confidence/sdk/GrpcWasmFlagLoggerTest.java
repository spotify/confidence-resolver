package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import com.spotify.confidence.sdk.flags.resolver.v1.*;
import io.grpc.ClientInterceptor;
import io.grpc.ManagedChannel;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;

class GrpcWasmFlagLoggerTest {

  @Test
  void testEmptyRequest_shouldSkip() {
    // Given
    final var mockStub =
        mock(InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub.class);
    final var logger = createLoggerWithMockStub(mockStub);
    final var emptyRequest = WriteFlagLogsRequest.newBuilder().build();

    // When
    logger.write(emptyRequest);

    // Then
    verify(mockStub, never()).clientWriteFlagLogs(any());
    logger.shutdown();
  }

  @Test
  void testSmallRequest_shouldSendAsIs() {
    // Given
    final var mockStub =
        mock(InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub.class);
    when(mockStub.clientWriteFlagLogs(any()))
        .thenReturn(WriteFlagLogsResponse.getDefaultInstance());
    final var logger = createLoggerWithMockStub(mockStub);

    final var request =
        WriteFlagLogsRequest.newBuilder()
            .addAllFlagAssigned(createFlagAssignedList(100))
            .addClientResolveInfo(
                ClientResolveInfo.newBuilder().setClient("clients/test-client").build())
            .addFlagResolveInfo(FlagResolveInfo.newBuilder().setFlag("flags/test-flag").build())
            .build();

    final ArgumentCaptor<WriteFlagLogsRequest> captor =
        ArgumentCaptor.forClass(WriteFlagLogsRequest.class);

    // When
    logger.write(request);

    // Then
    verify(mockStub, times(1)).clientWriteFlagLogs(captor.capture());

    final WriteFlagLogsRequest sentRequest = captor.getValue();
    assertEquals(100, sentRequest.getFlagAssignedCount());
    assertEquals(1, sentRequest.getClientResolveInfoCount());
    assertEquals(1, sentRequest.getFlagResolveInfoCount());

    logger.shutdown();
  }

  @Test
  void testLargeRequest_shouldChunkWithMetadataInFirstChunkOnly() {
    // Given
    final var mockStub =
        mock(InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub.class);
    when(mockStub.clientWriteFlagLogs(any()))
        .thenReturn(WriteFlagLogsResponse.getDefaultInstance());
    final var logger = createLoggerWithMockStub(mockStub);

    final int totalFlags = 2500; // Will create 3 chunks: 1000, 1000, 500
    final var request =
        WriteFlagLogsRequest.newBuilder()
            .addAllFlagAssigned(createFlagAssignedList(totalFlags))
            .setTelemetryData(TelemetryData.newBuilder().build())
            .addClientResolveInfo(
                ClientResolveInfo.newBuilder().setClient("clients/test-client").build())
            .addFlagResolveInfo(FlagResolveInfo.newBuilder().setFlag("flags/test-flag").build())
            .build();

    final ArgumentCaptor<WriteFlagLogsRequest> captor =
        ArgumentCaptor.forClass(WriteFlagLogsRequest.class);

    // When
    logger.write(request);

    // Then
    verify(mockStub, times(3)).clientWriteFlagLogs(captor.capture());

    final List<WriteFlagLogsRequest> sentRequests = captor.getAllValues();
    assertEquals(3, sentRequests.size());

    // First chunk: 1000 flag_assigned + metadata
    final WriteFlagLogsRequest firstChunk = sentRequests.get(0);
    assertEquals(1000, firstChunk.getFlagAssignedCount());
    assertTrue(firstChunk.hasTelemetryData());
    assertEquals(1, firstChunk.getClientResolveInfoCount());
    assertEquals("clients/test-client", firstChunk.getClientResolveInfo(0).getClient());
    assertEquals(1, firstChunk.getFlagResolveInfoCount());
    assertEquals("flags/test-flag", firstChunk.getFlagResolveInfo(0).getFlag());

    // Second chunk: 1000 flag_assigned only, no metadata
    final WriteFlagLogsRequest secondChunk = sentRequests.get(1);
    assertEquals(1000, secondChunk.getFlagAssignedCount());
    assertEquals(false, secondChunk.hasTelemetryData());
    assertEquals(0, secondChunk.getClientResolveInfoCount());
    assertEquals(0, secondChunk.getFlagResolveInfoCount());

    // Third chunk: 500 flag_assigned only, no metadata
    final WriteFlagLogsRequest thirdChunk = sentRequests.get(2);
    assertEquals(500, thirdChunk.getFlagAssignedCount());
    assertEquals(false, thirdChunk.hasTelemetryData());
    assertEquals(0, thirdChunk.getClientResolveInfoCount());
    assertEquals(0, thirdChunk.getFlagResolveInfoCount());

    logger.shutdown();
  }

  @Test
  void testExactlyAtChunkBoundary_shouldCreateTwoChunks() {
    // Given
    final var mockStub =
        mock(InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub.class);
    when(mockStub.clientWriteFlagLogs(any()))
        .thenReturn(WriteFlagLogsResponse.getDefaultInstance());
    final var logger = createLoggerWithMockStub(mockStub);

    final int totalFlags = 2000; // Exactly 2 chunks of 1000
    final var request =
        WriteFlagLogsRequest.newBuilder()
            .addAllFlagAssigned(createFlagAssignedList(totalFlags))
            .setTelemetryData(TelemetryData.newBuilder().build())
            .build();

    final ArgumentCaptor<WriteFlagLogsRequest> captor =
        ArgumentCaptor.forClass(WriteFlagLogsRequest.class);

    // When
    logger.write(request);

    // Then
    verify(mockStub, times(2)).clientWriteFlagLogs(captor.capture());

    final List<WriteFlagLogsRequest> sentRequests = captor.getAllValues();
    assertEquals(2, sentRequests.size());

    // First chunk with metadata
    assertEquals(1000, sentRequests.get(0).getFlagAssignedCount());
    assertTrue(sentRequests.get(0).hasTelemetryData());

    // Second chunk without metadata
    assertEquals(1000, sentRequests.get(1).getFlagAssignedCount());
    assertEquals(false, sentRequests.get(1).hasTelemetryData());

    logger.shutdown();
  }

  @Test
  void testOnlyMetadata_noFlagAssigned_shouldSendAsIs() {
    // Given
    final var mockStub =
        mock(InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub.class);
    when(mockStub.clientWriteFlagLogs(any()))
        .thenReturn(WriteFlagLogsResponse.getDefaultInstance());
    final var logger = createLoggerWithMockStub(mockStub);

    final var request =
        WriteFlagLogsRequest.newBuilder()
            .setTelemetryData(TelemetryData.newBuilder().build())
            .addClientResolveInfo(
                ClientResolveInfo.newBuilder().setClient("clients/test-client").build())
            .build();

    final ArgumentCaptor<WriteFlagLogsRequest> captor =
        ArgumentCaptor.forClass(WriteFlagLogsRequest.class);

    // When
    logger.write(request);

    // Then
    verify(mockStub, times(1)).clientWriteFlagLogs(captor.capture());

    final WriteFlagLogsRequest sentRequest = captor.getValue();
    assertEquals(0, sentRequest.getFlagAssignedCount());
    assertTrue(sentRequest.hasTelemetryData());
    assertEquals(1, sentRequest.getClientResolveInfoCount());

    logger.shutdown();
  }

  @Test
  void testShutdownWaitsForPendingWrites() throws InterruptedException {
    // Given
    final var mockStub =
        mock(InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub.class);
    final CountDownLatch writeLatch = new CountDownLatch(1);
    final AtomicInteger writeCount = new AtomicInteger(0);

    when(mockStub.clientWriteFlagLogs(any()))
        .thenAnswer(
            invocation -> {
              try {
                Thread.sleep(500);
                writeCount.incrementAndGet();
                return WriteFlagLogsResponse.getDefaultInstance();
              } finally {
                writeLatch.countDown();
              }
            });

    // Use a real GrpcWasmFlagLogger that creates async tasks
    final GrpcWasmFlagLogger logger =
        new GrpcWasmFlagLogger(
            "clientSecret", mockStub::clientWriteFlagLogs, Duration.ofSeconds(2)); // 2s timeout

    final var request =
        WriteFlagLogsRequest.newBuilder().addAllFlagAssigned(createFlagAssignedList(10)).build();

    // When
    logger.write(request);
    logger.shutdown();

    // Then
    assertTrue(writeLatch.await(100, TimeUnit.MILLISECONDS), "Write should have completed");
    assertEquals(1, writeCount.get(), "Write should have been called exactly once");
    verify(mockStub, times(1)).clientWriteFlagLogs(any());
  }

  @Test
  void testShutdownTimeoutWhenWriteTakesTooLong() throws InterruptedException {
    // Given
    final var mockStub =
        mock(InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub.class);
    final AtomicInteger writeCount = new AtomicInteger(0);

    when(mockStub.clientWriteFlagLogs(any()))
        .thenAnswer(
            invocation -> {
              Thread.sleep(5000); // 5 seconds, longer than timeout
              writeCount.incrementAndGet();
              return WriteFlagLogsResponse.getDefaultInstance();
            });

    final GrpcWasmFlagLogger logger =
        new GrpcWasmFlagLogger(
            "client-secret",
            mockStub::clientWriteFlagLogs,
            Duration.ofMillis(500)); // 500ms timeout

    final var request =
        WriteFlagLogsRequest.newBuilder().addAllFlagAssigned(createFlagAssignedList(10)).build();

    // When
    new Thread(
            new Runnable() {
              @Override
              public void run() {
                logger.write(request);
              }
            })
        .start();

    // Shutdown should complete within timeout + small buffer, not wait for full 5s write
    final long startTime = System.currentTimeMillis();
    logger.shutdown();
    final long duration = System.currentTimeMillis() - startTime;

    // Then
    assertTrue(
        duration < 2000, "Shutdown should timeout quickly (< 2s), but took " + duration + "ms");
    assertEquals(0, writeCount.get(), "Write should not have completed successfully");
  }

  @Test
  void testMultiplePendingWritesAllComplete() throws InterruptedException {
    // Given
    final var mockStub =
        mock(InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub.class);
    final AtomicInteger completedWrites = new AtomicInteger(0);
    final CountDownLatch allWritesComplete = new CountDownLatch(5);

    // Each write takes 100ms
    when(mockStub.clientWriteFlagLogs(any()))
        .thenAnswer(
            invocation -> {
              Thread.sleep(100);
              completedWrites.incrementAndGet();
              allWritesComplete.countDown();
              return WriteFlagLogsResponse.getDefaultInstance();
            });

    final GrpcWasmFlagLogger logger =
        new GrpcWasmFlagLogger(
            "clientSecret", mockStub::clientWriteFlagLogs, Duration.ofSeconds(3)); // 3s timeout

    final var request =
        WriteFlagLogsRequest.newBuilder().addAllFlagAssigned(createFlagAssignedList(10)).build();

    // When - submit 5 async writes
    for (int i = 0; i < 5; i++) {
      logger.write(request);
    }

    // Shutdown should wait for all writes to complete
    logger.shutdown();

    // Then
    assertTrue(
        allWritesComplete.await(100, TimeUnit.MILLISECONDS),
        "All writes should have completed before shutdown returned");
    assertEquals(5, completedWrites.get(), "All 5 writes should have completed");
    verify(mockStub, times(5)).clientWriteFlagLogs(any());
  }

  @Test
  void testShutdownClosesChannel() throws Exception {
    final ManagedChannel mockChannel = mock(ManagedChannel.class);
    final ChannelFactory channelFactory =
        new ChannelFactory() {
          @Override
          public ManagedChannel create(String target, List<ClientInterceptor> defaultInterceptors) {
            return mockChannel;
          }
        };

    final GrpcWasmFlagLogger logger = new GrpcWasmFlagLogger("test-client-secret", channelFactory);
    logger.shutdown();
    verify(mockChannel, times(1)).shutdown();
  }

  // Helper methods

  private List<FlagAssigned> createFlagAssignedList(int count) {
    final List<FlagAssigned> list = new ArrayList<>();
    for (int i = 0; i < count; i++) {
      list.add(
          FlagAssigned.newBuilder()
              .setResolveId("resolve-" + i)
              .setClientInfo(
                  ClientInfo.newBuilder()
                      .setClient("clients/test-client")
                      .setClientCredential("clients/test-client/credentials/cred-1")
                      .build())
              .addFlags(
                  FlagAssigned.AppliedFlag.newBuilder()
                      .setFlag("flags/test-flag-" + i)
                      .setTargetingKey("user-" + i)
                      .setAssignmentId("assignment-" + i)
                      .build())
              .build());
    }
    return list;
  }

  private GrpcWasmFlagLogger createLoggerWithMockStub(
      InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub mockStub) {
    // Create logger with synchronous test writer
    return new GrpcWasmFlagLogger("test-client-secret", mockStub::clientWriteFlagLogs);
  }
}
