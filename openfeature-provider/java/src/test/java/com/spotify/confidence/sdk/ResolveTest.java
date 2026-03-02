package com.spotify.confidence.sdk;

import static org.assertj.core.api.AssertionsForClassTypes.assertThatExceptionOfType;
import static org.assertj.core.api.AssertionsForInterfaceTypes.assertThat;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.*;

import com.google.protobuf.Struct;
import com.google.protobuf.util.Structs;
import com.google.protobuf.util.Values;
import com.spotify.confidence.sdk.flags.admin.v1.Client;
import com.spotify.confidence.sdk.flags.admin.v1.ClientCredential;
import com.spotify.confidence.sdk.flags.admin.v1.Flag;
import com.spotify.confidence.sdk.flags.admin.v1.Segment;
import com.spotify.confidence.sdk.flags.resolver.v1.*;
import com.spotify.confidence.sdk.flags.types.v1.FlagSchema;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.stream.Collectors;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

class ResolveTest {

  protected final MaterializationStore mockMaterializationStore = mock(MaterializationStore.class);
  protected static final ClientCredential.ClientSecret secret =
      ClientCredential.ClientSecret.newBuilder().setSecret("very-secret").build();
  static final String clientName = "clients/client";
  static final String credentialName = clientName + "/credentials/creddy";
  private final LocalResolver resolver;

  public ResolveTest() {
    resolver = new WasmLocalResolver(request -> {});
  }

  @BeforeEach
  void setUp() {
    useStateWithoutFlagsWithMaterialization();
  }

  private static final String ACCOUNT = "account";
  private static final String flag1 = "flags/flag-1";

  private static final String flagOff = flag1 + "/variants/offf";
  private static final String flagOn = flag1 + "/variants/onnn";

  private static final Flag.Variant variantOff =
      Flag.Variant.newBuilder()
          .setName(flagOff)
          .setValue(Structs.of("data", Values.of("off")))
          .build();
  private static final Flag.Variant variantOn =
      Flag.Variant.newBuilder()
          .setName(flagOn)
          .setValue(Structs.of("data", Values.of("on")))
          .build();

  private static final FlagSchema.StructFlagSchema schema1 =
      FlagSchema.StructFlagSchema.newBuilder()
          .putSchema(
              "data",
              FlagSchema.newBuilder()
                  .setStringSchema(FlagSchema.StringFlagSchema.newBuilder().build())
                  .build())
          .putSchema(
              "extra",
              FlagSchema.newBuilder()
                  .setStringSchema(FlagSchema.StringFlagSchema.newBuilder().build())
                  .build())
          .build();
  private static final String segmentA = "segments/seg-a";
  static final byte[] exampleStateBytes;
  static final byte[] exampleStateWithMaterializationBytes;
  private static final Map<String, Flag> flags =
      Map.of(
          flag1,
          Flag.newBuilder()
              .setName(flag1)
              .setState(Flag.State.ACTIVE)
              .setSchema(schema1)
              .addVariants(variantOff)
              .addVariants(variantOn)
              .addClients(clientName)
              .addRules(
                  Flag.Rule.newBuilder()
                      .setName("MyRule")
                      .setSegment(segmentA)
                      .setEnabled(true)
                      .setAssignmentSpec(
                          Flag.Rule.AssignmentSpec.newBuilder()
                              .setBucketCount(2)
                              .addAssignments(
                                  Flag.Rule.Assignment.newBuilder()
                                      .setAssignmentId(flagOff)
                                      .setVariant(
                                          Flag.Rule.Assignment.VariantAssignment.newBuilder()
                                              .setVariant(flagOff)
                                              .build())
                                      .addBucketRanges(
                                          Flag.Rule.BucketRange.newBuilder()
                                              .setLower(0)
                                              .setUpper(1)
                                              .build())
                                      .build())
                              .addAssignments(
                                  Flag.Rule.Assignment.newBuilder()
                                      .setAssignmentId(flagOn)
                                      .setVariant(
                                          Flag.Rule.Assignment.VariantAssignment.newBuilder()
                                              .setVariant(flagOn)
                                              .build())
                                      .addBucketRanges(
                                          Flag.Rule.BucketRange.newBuilder()
                                              .setLower(1)
                                              .setUpper(2)
                                              .build())
                                      .build())
                              .build())
                      .build())
              .build());

  private static final Map<String, Flag> flagsWithMaterialization =
      Map.of(
          flag1,
          Flag.newBuilder()
              .setName(flag1)
              .setState(Flag.State.ACTIVE)
              .setSchema(schema1)
              .addVariants(variantOff)
              .addVariants(variantOn)
              .addClients(clientName)
              .addRules(
                  Flag.Rule.newBuilder()
                      .setName("MyRule")
                      .setSegment(segmentA)
                      .setEnabled(true)
                      .setMaterializationSpec(
                          Flag.Rule.MaterializationSpec.newBuilder()
                              .setReadMaterialization("read-mat")
                              .setMode(
                                  Flag.Rule.MaterializationSpec.MaterializationReadMode.newBuilder()
                                      .setMaterializationMustMatch(
                                          false) // true means the intake is paused. false means we
                                      // accept new assignments
                                      .setSegmentTargetingCanBeIgnored(false)
                                      .build())
                              .setWriteMaterialization("write-mat")
                              .build())
                      .setAssignmentSpec(
                          Flag.Rule.AssignmentSpec.newBuilder()
                              .setBucketCount(2)
                              .addAssignments(
                                  Flag.Rule.Assignment.newBuilder()
                                      .setAssignmentId(flagOff)
                                      .setVariant(
                                          Flag.Rule.Assignment.VariantAssignment.newBuilder()
                                              .setVariant(flagOff)
                                              .build())
                                      .addBucketRanges(
                                          Flag.Rule.BucketRange.newBuilder()
                                              .setLower(0)
                                              .setUpper(1)
                                              .build())
                                      .build())
                              .addAssignments(
                                  Flag.Rule.Assignment.newBuilder()
                                      .setAssignmentId(flagOn)
                                      .setVariant(
                                          Flag.Rule.Assignment.VariantAssignment.newBuilder()
                                              .setVariant(flagOn)
                                              .build())
                                      .addBucketRanges(
                                          Flag.Rule.BucketRange.newBuilder()
                                              .setLower(1)
                                              .setUpper(2)
                                              .build())
                                      .build())
                              .build())
                      .build())
              .build());
  protected static final Map<String, Segment> segments =
      Map.of(segmentA, Segment.newBuilder().setName(segmentA).build());

  static {
    exampleStateBytes = buildResolverStateBytes(flags);
    exampleStateWithMaterializationBytes = buildResolverStateBytes(flagsWithMaterialization);
  }

  protected void useStateWithFlagsWithMaterialization() {
    resolver.setResolverState(exampleStateWithMaterializationBytes, ACCOUNT);
  }

  protected void useStateWithoutFlagsWithMaterialization() {
    resolver.setResolverState(exampleStateBytes, ACCOUNT);
  }

  @Test
  public void testInvalidSecret() {
    assertThatExceptionOfType(RuntimeException.class)
        .isThrownBy(
            () ->
                resolveWithContext(
                    List.of("flags/asd"),
                    "foo",
                    Struct.newBuilder().build(),
                    true,
                    "invalid-secret"))
        .withMessage("client secret not found");
  }

  @Test
  public void testInvalidFlag() {
    final var response =
        resolveWithContext(List.of("flags/asd"), "foo", Struct.newBuilder().build(), false);
    assertThat(response.getResolvedFlagsList()).isEmpty();
    assertThat(response.getResolveId()).isNotEmpty();
  }

  @Test
  public void testResolveFlag() {
    final var response =
        resolveWithContext(List.of(flag1), "foo", Struct.newBuilder().build(), true);
    assertThat(response.getResolveId()).isNotEmpty();
    final Struct expectedValue = variantOn.getValue();

    assertEquals(variantOn.getName(), response.getResolvedFlags(0).getVariant());
    assertEquals(expectedValue, response.getResolvedFlags(0).getValue());
    assertEquals(schema1, response.getResolvedFlags(0).getFlagSchema());
  }

  @Test
  public void testResolveFlagWithEncryptedResolveToken() {
    final var response =
        resolveWithContext(List.of(flag1), "foo", Struct.newBuilder().build(), false);
    assertThat(response.getResolveId()).isNotEmpty();
    final Struct expectedValue = variantOn.getValue();

    assertEquals(variantOn.getName(), response.getResolvedFlags(0).getVariant());
    assertEquals(expectedValue, response.getResolvedFlags(0).getValue());
    assertEquals(schema1, response.getResolvedFlags(0).getFlagSchema());
    assertThat(response.getResolveToken()).isNotEmpty();
  }

  @Test
  public void testResolveFlagWithMaterializationsWithUnsupportedStore() {
    useStateWithFlagsWithMaterialization();

    // Attempting to resolve a flag that requires materializations with UnsupportedStore
    // should throw MaterializationNotSupportedException
    assertThatExceptionOfType(MaterializationNotSupportedException.class)
        .isThrownBy(
            () ->
                resolveWithMaterializations(
                    List.of(flag1),
                    "foo",
                    Struct.newBuilder().build(),
                    true,
                    secret.getSecret(),
                    new UnsupportedMaterializationStore()));
  }

  @Test
  public void testResolveFlagWithMaterializationsWithMockedStoreContainingVariant() {
    useStateWithFlagsWithMaterialization();
    when(mockMaterializationStore.write(any())).thenReturn(CompletableFuture.completedFuture(null));
    when(mockMaterializationStore.read(
            argThat(
                (arg) ->
                    arg.get(0).materialization().equalsIgnoreCase("read-mat")
                        && arg.get(0).unit().equalsIgnoreCase("foo"))))
        .thenReturn(
            CompletableFuture.completedFuture(
                List.of(
                    new MaterializationStore.ReadResult.Variant(
                        "read-mat", "foo", "MyRule", Optional.of(flagOn)))));
    ResolveFlagsResponse response =
        resolveWithMaterializations(
            List.of(flag1),
            "foo",
            Struct.newBuilder().build(),
            true,
            secret.getSecret(),
            mockMaterializationStore);

    final Struct expectedValue = variantOn.getValue();
    assertEquals(variantOn.getName(), response.getResolvedFlags(0).getVariant());
    assertEquals(expectedValue, response.getResolvedFlags(0).getValue());
    assertEquals(schema1, response.getResolvedFlags(0).getFlagSchema());
  }

  @Test
  public void testResolveFlagWithMaterializationsWithMockedStoreNotContainingVariant() {
    useStateWithFlagsWithMaterialization();
    when(mockMaterializationStore.write(any())).thenReturn(CompletableFuture.completedFuture(null));
    when(mockMaterializationStore.read(
            argThat(
                (arg) ->
                    arg.get(0).materialization().equalsIgnoreCase("read-mat")
                        && arg.get(0).unit().equalsIgnoreCase("foo"))))
        .thenReturn(
            CompletableFuture.completedFuture(
                List.of(
                    new MaterializationStore.ReadResult.Variant(
                        "read-mat", "foo", "MyRule", Optional.empty()))));
    ResolveFlagsResponse response =
        resolveWithMaterializations(
            List.of(flag1),
            "foo",
            Struct.newBuilder().build(),
            true,
            secret.getSecret(),
            mockMaterializationStore);
    verify(mockMaterializationStore)
        .write(
            argThat(
                set -> {
                  MaterializationStore.WriteOp.Variant writeOp =
                      (MaterializationStore.WriteOp.Variant) set.stream().toList().get(0);
                  return set.size() == 1
                      && writeOp.unit().equalsIgnoreCase("foo")
                      && writeOp.materialization().equalsIgnoreCase("write-mat")
                      && writeOp.rule().equalsIgnoreCase("MyRule")
                      && writeOp.variant().equalsIgnoreCase(flagOn);
                }));

    final Struct expectedValue = variantOn.getValue();
    assertEquals(variantOn.getName(), response.getResolvedFlags(0).getVariant());
    assertEquals(expectedValue, response.getResolvedFlags(0).getValue());
    assertEquals(schema1, response.getResolvedFlags(0).getFlagSchema());
  }

  @Test
  public void testResolveFlagWithMaterializationsIsProtectedAgainstInfiniteRecursion() {
    useStateWithFlagsWithMaterialization();
    when(mockMaterializationStore.write(any())).thenReturn(CompletableFuture.completedFuture(null));
    when(mockMaterializationStore.read(
            argThat(
                (arg) ->
                    arg.get(0).materialization().equalsIgnoreCase("read-mat")
                        && arg.get(0).unit().equalsIgnoreCase("foo"))))
        .thenReturn(
            CompletableFuture.completedFuture(
                List.of())); // this will cause recursion in the resolve.
    try {
      resolveWithMaterializations(
          List.of(flag1),
          "foo",
          Struct.newBuilder().build(),
          true,
          secret.getSecret(),
          mockMaterializationStore);
    } catch (Exception e) {
      assertThat(e).isInstanceOf(RuntimeException.class);
      assertThat(e.getMessage()).contains("Unexpected second suspend after resume");
    }
  }

  @Test
  public void testTooLongKey() {
    // Targeting key > 100 chars results in a TargetingKeyError reason, not an exception
    ResolveFlagsResponse response =
        resolveWithContext(List.of(flag1), "a".repeat(101), Struct.newBuilder().build(), false);
    assertEquals(
        ResolveReason.RESOLVE_REASON_TARGETING_KEY_ERROR, response.getResolvedFlags(0).getReason());
  }

  @Test
  public void testResolveIntegerTargetingKeyTyped() {
    final var response =
        resolveWithNumericTargetingKey(List.of(flag1), 1234567890, Struct.newBuilder().build());

    assertThat(response.getResolvedFlagsList()).hasSize(1);
    assertEquals(ResolveReason.RESOLVE_REASON_MATCH, response.getResolvedFlags(0).getReason());
  }

  @Test
  public void testResolveDecimalUsername() {
    final var response =
        resolveWithNumericTargetingKey(List.of(flag1), 3.14159d, Struct.newBuilder().build());

    assertThat(response.getResolvedFlagsList()).hasSize(1);
    assertEquals(
        ResolveReason.RESOLVE_REASON_TARGETING_KEY_ERROR, response.getResolvedFlags(0).getReason());
  }

  private static byte[] buildResolverStateBytes(Map<String, Flag> flagsMap) {
    final var builder = com.spotify.confidence.sdk.flags.admin.v1.ResolverState.newBuilder();
    builder.addAllFlags(flagsMap.values());
    builder.addAllSegmentsNoBitsets(segments.values());
    // All-one bitset for each segment
    segments
        .keySet()
        .forEach(
            name ->
                builder.addBitsets(
                    com.spotify.confidence.sdk.flags.admin.v1.ResolverState.PackedBitset
                        .newBuilder()
                        .setSegment(name)
                        .setFullBitset(true)
                        .build()));
    builder.addClients(Client.newBuilder().setName(clientName).build());
    builder.addClientCredentials(
        ClientCredential.newBuilder().setName(credentialName).setClientSecret(secret).build());
    return builder.build().toByteArray();
  }

  /** Resolve without materialization support (simple DeferredMaterializations path). */
  private ResolveFlagsResponse resolveWithContext(
      List<String> flags, String username, Struct struct, boolean apply, String secret) {
    final var request =
        ResolveProcessRequest.newBuilder()
            .setDeferredMaterializations(
                ResolveFlagsRequest.newBuilder()
                    .addAllFlags(flags)
                    .setClientSecret(secret)
                    .setEvaluationContext(
                        Structs.of("targeting_key", Values.of(username), "bar", Values.of(struct)))
                    .setApply(apply)
                    .build())
            .build();
    final var response = resolver.resolveProcess(request);
    // For non-materialization tests, just extract resolved response directly
    if (response.hasResolved()) {
      return response.getResolved().getResponse();
    }
    throw new RuntimeException("Unexpected response: " + response.getResultCase());
  }

  /**
   * Resolve with materialization suspend/resume support, mirroring the provider's
   * resolveWithMaterializations logic.
   */
  private ResolveFlagsResponse resolveWithMaterializations(
      List<String> flags,
      String username,
      Struct struct,
      boolean apply,
      String secret,
      MaterializationStore store) {
    final var request =
        ResolveProcessRequest.newBuilder()
            .setDeferredMaterializations(
                ResolveFlagsRequest.newBuilder()
                    .addAllFlags(flags)
                    .setClientSecret(secret)
                    .setEvaluationContext(
                        Structs.of("targeting_key", Values.of(username), "bar", Values.of(struct)))
                    .setApply(apply)
                    .build())
            .build();
    final var response = resolver.resolveProcess(request);
    return handleResponse(response, store);
  }

  private ResolveFlagsResponse handleResponse(
      ResolveProcessResponse response, MaterializationStore store) {
    switch (response.getResultCase()) {
      case RESOLVED -> {
        final var resolved = response.getResolved();
        if (!resolved.getMaterializationsToWriteList().isEmpty()) {
          final Set<MaterializationStore.WriteOp> writeOps =
              resolved.getMaterializationsToWriteList().stream()
                  .map(
                      r ->
                          new MaterializationStore.WriteOp.Variant(
                              r.getMaterialization(), r.getUnit(), r.getRule(), r.getVariant()))
                  .collect(Collectors.toSet());
          store.write(writeOps).toCompletableFuture().join();
        }
        return resolved.getResponse();
      }
      case SUSPENDED -> {
        final var suspended = response.getSuspended();
        final var resumeRequest = handleSuspended(suspended, store);
        final var resumeResponse = resolver.resolveProcess(resumeRequest);
        return handleResumeResponse(resumeResponse, store);
      }
      default -> throw new RuntimeException("Unexpected response: " + response.getResultCase());
    }
  }

  private ResolveFlagsResponse handleResumeResponse(
      ResolveProcessResponse response, MaterializationStore store) {
    switch (response.getResultCase()) {
      case RESOLVED -> {
        final var resolved = response.getResolved();
        if (!resolved.getMaterializationsToWriteList().isEmpty()) {
          final Set<MaterializationStore.WriteOp> writeOps =
              resolved.getMaterializationsToWriteList().stream()
                  .map(
                      r ->
                          new MaterializationStore.WriteOp.Variant(
                              r.getMaterialization(), r.getUnit(), r.getRule(), r.getVariant()))
                  .collect(Collectors.toSet());
          store.write(writeOps).toCompletableFuture().join();
        }
        return resolved.getResponse();
      }
      case SUSPENDED -> throw new RuntimeException("Unexpected second suspend after resume");
      default ->
          throw new RuntimeException(
              "Unhandled response case after resume: " + response.getResultCase());
    }
  }

  private ResolveProcessRequest handleSuspended(
      ResolveProcessResponse.Suspended suspended, MaterializationStore store) {
    final var toRead = suspended.getMaterializationsToReadList();
    final List<? extends MaterializationStore.ReadOp> readOps =
        toRead.stream()
            .map(
                record -> {
                  if (!record.getRule().isEmpty()) {
                    return new MaterializationStore.ReadOp.Variant(
                        record.getMaterialization(), record.getUnit(), record.getRule());
                  } else {
                    return new MaterializationStore.ReadOp.Inclusion(
                        record.getMaterialization(), record.getUnit());
                  }
                })
            .toList();

    final var results = store.read(readOps).toCompletableFuture().join();
    final var materializations =
        results.stream()
            .flatMap(
                rr -> {
                  if (rr instanceof MaterializationStore.ReadResult.Variant variant) {
                    if (variant.variant().isPresent()) {
                      return java.util.stream.Stream.of(
                          MaterializationRecord.newBuilder()
                              .setUnit(variant.unit())
                              .setMaterialization(variant.materialization())
                              .setRule(variant.rule())
                              .setVariant(variant.variant().get())
                              .build());
                    }
                  }
                  if (rr instanceof MaterializationStore.ReadResult.Inclusion inclusion) {
                    if (inclusion.included()) {
                      return java.util.stream.Stream.of(
                          MaterializationRecord.newBuilder()
                              .setUnit(inclusion.unit())
                              .setMaterialization(inclusion.materialization())
                              .build());
                    }
                  }
                  return java.util.stream.Stream.empty();
                })
            .toList();

    return ResolveProcessRequest.newBuilder()
        .setResume(
            ResolveProcessRequest.Resume.newBuilder()
                .addAllMaterializations(materializations)
                .setState(suspended.getState()))
        .build();
  }

  private ResolveFlagsResponse resolveWithNumericTargetingKey(
      List<String> flags, Number targetingKey, Struct struct) {

    final var builder =
        ResolveFlagsRequest.newBuilder()
            .addAllFlags(flags)
            .setClientSecret(secret.getSecret())
            .setApply(true);

    if (targetingKey instanceof Double || targetingKey instanceof Float) {
      builder.setEvaluationContext(
          Structs.of(
              "targeting_key", Values.of(targetingKey.doubleValue()), "bar", Values.of(struct)));
    } else {
      builder.setEvaluationContext(
          Structs.of(
              "targeting_key", Values.of(targetingKey.longValue()), "bar", Values.of(struct)));
    }

    final var request =
        ResolveProcessRequest.newBuilder().setDeferredMaterializations(builder.build()).build();
    final var response = resolver.resolveProcess(request);
    if (response.hasResolved()) {
      return response.getResolved().getResponse();
    }
    throw new RuntimeException("Unexpected response: " + response.getResultCase());
  }

  private ResolveFlagsResponse resolveWithContext(
      List<String> flags, String username, Struct struct, boolean apply) {
    return resolveWithContext(flags, username, struct, apply, secret.getSecret());
  }
}
