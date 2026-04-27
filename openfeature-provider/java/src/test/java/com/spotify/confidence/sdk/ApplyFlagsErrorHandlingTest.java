package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatNoException;

import ch.qos.logback.classic.Level;
import ch.qos.logback.classic.Logger;
import ch.qos.logback.classic.spi.ILoggingEvent;
import ch.qos.logback.core.read.ListAppender;
import com.google.protobuf.Timestamp;
import com.google.protobuf.util.Structs;
import com.google.protobuf.util.Values;
import com.spotify.confidence.sdk.flags.admin.v1.Client;
import com.spotify.confidence.sdk.flags.admin.v1.ClientCredential;
import com.spotify.confidence.sdk.flags.admin.v1.Flag;
import com.spotify.confidence.sdk.flags.admin.v1.ResolverState;
import com.spotify.confidence.sdk.flags.admin.v1.Segment;
import com.spotify.confidence.sdk.flags.resolver.v1.AppliedFlag;
import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsResponse;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import com.spotify.confidence.sdk.flags.types.v1.FlagSchema;
import java.time.Instant;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.slf4j.LoggerFactory;

/**
 * Verifies the contract for {@link WasmLocalResolver#applyFlags}:
 *
 * <ul>
 *   <li>A malformed apply request (e.g. missing top-level {@code send_time}) does NOT propagate as
 *       an exception — apply is best-effort logging — but no FlagAssigned is recorded.
 *   <li>A well-formed apply request results in exactly one FlagAssigned entry on the next flush.
 * </ul>
 */
class ApplyFlagsErrorHandlingTest {
  private ListAppender<ILoggingEvent> logAppender;
  private Logger wasmLogger;

  @BeforeEach
  void attachLogAppender() {
    logAppender = new ListAppender<>();
    logAppender.start();
    wasmLogger = (Logger) LoggerFactory.getLogger(WasmLocalResolver.class);
    wasmLogger.addAppender(logAppender);
  }

  @AfterEach
  void detachLogAppender() {
    if (wasmLogger != null && logAppender != null) {
      wasmLogger.detachAppender(logAppender);
    }
  }

  private static final String SECRET = "very-secret";
  private static final String ACCOUNT = "account";
  private static final String CLIENT_NAME = "clients/c";
  private static final String CRED_NAME = CLIENT_NAME + "/credentials/cred";
  private static final String FLAG_NAME = "flags/test";
  private static final String SEGMENT_NAME = "segments/seg";
  private static final String VARIANT_ON = FLAG_NAME + "/variants/on";

  private static byte[] buildState() {
    return buildState(SECRET);
  }

  private static byte[] buildState(String secret) {
    final var schema =
        FlagSchema.StructFlagSchema.newBuilder()
            .putSchema(
                "b",
                FlagSchema.newBuilder()
                    .setBoolSchema(FlagSchema.BoolFlagSchema.newBuilder().build())
                    .build())
            .build();
    final var flag =
        Flag.newBuilder()
            .setName(FLAG_NAME)
            .setState(Flag.State.ACTIVE)
            .setSchema(schema)
            .addVariants(
                Flag.Variant.newBuilder()
                    .setName(VARIANT_ON)
                    .setValue(Structs.of("b", Values.of(true)))
                    .build())
            .addClients(CLIENT_NAME)
            .addRules(
                Flag.Rule.newBuilder()
                    .setName("rule")
                    .setSegment(SEGMENT_NAME)
                    .setEnabled(true)
                    .setAssignmentSpec(
                        Flag.Rule.AssignmentSpec.newBuilder()
                            .setBucketCount(1)
                            .addAssignments(
                                Flag.Rule.Assignment.newBuilder()
                                    .setAssignmentId(VARIANT_ON)
                                    .setVariant(
                                        Flag.Rule.Assignment.VariantAssignment.newBuilder()
                                            .setVariant(VARIANT_ON)
                                            .build())
                                    .addBucketRanges(
                                        Flag.Rule.BucketRange.newBuilder()
                                            .setLower(0)
                                            .setUpper(1)
                                            .build())
                                    .build())
                            .build())
                    .build())
            .build();
    return ResolverState.newBuilder()
        .addFlags(flag)
        .addSegmentsNoBitsets(Segment.newBuilder().setName(SEGMENT_NAME).build())
        .addBitsets(
            ResolverState.PackedBitset.newBuilder()
                .setSegment(SEGMENT_NAME)
                .setFullBitset(true)
                .build())
        .addClients(Client.newBuilder().setName(CLIENT_NAME).build())
        .addClientCredentials(
            ClientCredential.newBuilder()
                .setName(CRED_NAME)
                .setClientSecret(
                    ClientCredential.ClientSecret.newBuilder().setSecret(secret).build())
                .build())
        .build()
        .toByteArray();
  }

  private static ResolveFlagsResponse resolveWithApplyFalse(WasmLocalResolver r) {
    final var request =
        ResolveProcessRequest.newBuilder()
            .setDeferredMaterializations(
                ResolveFlagsRequest.newBuilder()
                    .addFlags(FLAG_NAME)
                    .setClientSecret(SECRET)
                    .setEvaluationContext(Structs.of("targeting_key", Values.of("user-1")))
                    .setApply(false)
                    .build())
            .build();
    return r.resolveProcess(request).toCompletableFuture().join().getResolved().getResponse();
  }

  private static Timestamp toTs(Instant t) {
    return Timestamp.newBuilder().setSeconds(t.getEpochSecond()).setNanos(t.getNano()).build();
  }

  @Test
  void applyWithoutSendTime_isSwallowed_andRecordsNoFlagAssigned() {
    final List<WriteFlagLogsRequest> captured = new ArrayList<>();
    final var resolver = new WasmLocalResolver(captured::add);
    resolver.setResolverState(buildState(), ACCOUNT, null);

    final var resolveResp = resolveWithApplyFalse(resolver);
    final var malformed =
        ApplyFlagsRequest.newBuilder()
            .setClientSecret(SECRET)
            .setResolveToken(resolveResp.getResolveToken())
            .addFlags(AppliedFlag.newBuilder().setFlag(FLAG_NAME).setApplyTime(toTs(Instant.now())))
            // no setSendTime(...) — invalid; resolver core requires it
            .build();

    assertThatNoException().isThrownBy(() -> resolver.applyFlags(malformed));
    resolver.flushAllLogs();

    final int totalAssigned =
        captured.stream().mapToInt(WriteFlagLogsRequest::getFlagAssignedCount).sum();
    assertThat(totalAssigned).isZero();

    assertThat(logAppender.list)
        .as("apply failure is surfaced as a WARN log")
        .anyMatch(
            e ->
                e.getLevel() == Level.WARN
                    && e.getFormattedMessage().contains("Failed to apply flags")
                    && e.getFormattedMessage().contains("send_time is required"));
  }

  @Test
  void applyWithSendTime_recordsOneFlagAssigned() {
    final List<WriteFlagLogsRequest> captured = new ArrayList<>();
    final var resolver = new WasmLocalResolver(captured::add);
    resolver.setResolverState(buildState(), ACCOUNT, null);

    final var resolveResp = resolveWithApplyFalse(resolver);
    final var now = toTs(Instant.now());
    final var wellFormed =
        ApplyFlagsRequest.newBuilder()
            .setClientSecret(SECRET)
            .setResolveToken(resolveResp.getResolveToken())
            .setSendTime(now)
            .addFlags(AppliedFlag.newBuilder().setFlag(FLAG_NAME).setApplyTime(now))
            .build();

    resolver.applyFlags(wellFormed);
    resolver.flushAllLogs();

    final var assignedEvents =
        captured.stream().flatMap(r -> r.getFlagAssignedList().stream()).toList();
    assertThat(assignedEvents).hasSize(1);

    final var assigned = assignedEvents.get(0);
    assertThat(assigned.getResolveId()).isEqualTo(resolveResp.getResolveId());
    assertThat(assigned.hasClientInfo()).isTrue();
    assertThat(assigned.getClientInfo().getClient()).isEqualTo(CLIENT_NAME);
    assertThat(assigned.getClientInfo().getClientCredential()).isEqualTo(CRED_NAME);

    assertThat(assigned.getFlagsList()).hasSize(1);
    final var appliedFlag = assigned.getFlags(0);
    assertThat(appliedFlag.getFlag()).isEqualTo(FLAG_NAME);
    assertThat(appliedFlag.getTargetingKey()).isEqualTo("user-1");
    assertThat(appliedFlag.hasAssignmentInfo()).isTrue();
    assertThat(appliedFlag.getAssignmentInfo().getSegment()).isEqualTo(SEGMENT_NAME);
    assertThat(appliedFlag.getAssignmentInfo().getVariant()).isEqualTo(VARIANT_ON);
    assertThat(appliedFlag.getAssignmentId()).isEqualTo(VARIANT_ON);
    assertThat(appliedFlag.hasApplyTime()).isTrue();

    assertThat(logAppender.list).noneMatch(e -> e.getLevel() == Level.WARN);
  }

  /**
   * Resolver state can be refreshed between resolve and apply (e.g. by the periodic state-poll
   * thread). If the new state no longer carries the client_secret used at resolve time, the apply
   * cannot be authenticated. This must not crash the caller — apply is best-effort logging — but
   * the failure should be visible in logs.
   */
  @Test
  void applyAfterStateRotated_secretRemoved_isSwallowedAndLogged() {
    final List<WriteFlagLogsRequest> captured = new ArrayList<>();
    final var resolver = new WasmLocalResolver(captured::add);
    resolver.setResolverState(buildState(SECRET), ACCOUNT, null);

    final var resolveResp = resolveWithApplyFalse(resolver);

    // State rotates — the original secret is no longer present.
    resolver.setResolverState(buildState("rotated-secret"), ACCOUNT, null);

    final var now = toTs(Instant.now());
    final var apply =
        ApplyFlagsRequest.newBuilder()
            .setClientSecret(SECRET) // resolved-against secret, no longer in state
            .setResolveToken(resolveResp.getResolveToken())
            .setSendTime(now)
            .addFlags(AppliedFlag.newBuilder().setFlag(FLAG_NAME).setApplyTime(now))
            .build();

    assertThatNoException().isThrownBy(() -> resolver.applyFlags(apply));
    resolver.flushAllLogs();

    final int totalAssigned =
        captured.stream().mapToInt(WriteFlagLogsRequest::getFlagAssignedCount).sum();
    assertThat(totalAssigned).isZero();

    assertThat(logAppender.list)
        .as("apply against rotated state surfaces a WARN log")
        .anyMatch(
            e ->
                e.getLevel() == Level.WARN
                    && e.getFormattedMessage().contains("Failed to apply flags")
                    && e.getFormattedMessage().contains("client secret not found"));
  }
}
