package com.spotify.confidence.sdk;

import static org.assertj.core.api.AssertionsForClassTypes.assertThatExceptionOfType;
import static org.assertj.core.api.AssertionsForInterfaceTypes.assertThat;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.*;

import com.google.protobuf.Struct;
import com.google.protobuf.util.Structs;
import com.google.protobuf.util.Values;
import com.spotify.confidence.flags.admin.v1.Client;
import com.spotify.confidence.flags.admin.v1.ClientCredential;
import com.spotify.confidence.flags.admin.v1.Flag;
import com.spotify.confidence.flags.admin.v1.Segment;
import com.spotify.confidence.flags.resolver.v1.*;
import com.spotify.confidence.flags.types.v1.FlagSchema;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

class ResolveTest {

  protected final MaterializationStore mockMaterializationStore = mock(MaterializationStore.class);
  protected static final ClientCredential.ClientSecret secret =
      ClientCredential.ClientSecret.newBuilder().setSecret("very-secret").build();
  static final String clientName = "clients/client";
  static final String credentialName = clientName + "/credentials/creddy";
  private final ResolverApi resolverApi;

  public ResolveTest() {
    final var wasmResolverApi =
        new SwapWasmResolverApi(
            new WasmFlagLogger() {
              @Override
              public void write(WriteFlagLogsRequest request) {}

              @Override
              public void writeSync(WriteFlagLogsRequest request) {}

              @Override
              public void shutdown() {}
            },
            new byte[0],
            "",
            mockMaterializationStore);
    this.resolverApi = wasmResolverApi;
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
    resolverApi.updateStateAndFlushLogs(exampleStateWithMaterializationBytes, ACCOUNT);
  }

  protected void useStateWithoutFlagsWithMaterialization() {
    resolverApi.updateStateAndFlushLogs(exampleStateBytes, ACCOUNT);
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
    // Use the UnsupportedMaterializationStore to test default behavior
    final var wasmResolverApi =
        new SwapWasmResolverApi(
            new WasmFlagLogger() {
              @Override
              public void write(WriteFlagLogsRequest request) {}

              @Override
              public void writeSync(WriteFlagLogsRequest request) {}

              @Override
              public void shutdown() {}
            },
            new byte[0],
            "",
            new UnsupportedMaterializationStore());

    wasmResolverApi.updateStateAndFlushLogs(exampleStateWithMaterializationBytes, ACCOUNT);

    // Attempting to resolve a flag that requires materializations should throw
    // MaterializationNotSupportedException
    assertThatExceptionOfType(MaterializationNotSupportedException.class)
        .isThrownBy(
            () ->
                wasmResolverApi
                    .resolveWithSticky(
                        ResolveWithStickyRequest.newBuilder()
                            .setFailFastOnSticky(false)
                            .setResolveRequest(
                                ResolveFlagsRequest.newBuilder()
                                    .addAllFlags(List.of(flag1))
                                    .setClientSecret(secret.getSecret())
                                    .setEvaluationContext(
                                        Structs.of(
                                            "targeting_key",
                                            Values.of("foo"),
                                            "bar",
                                            Values.of(Struct.newBuilder().build())))
                                    .setApply(true))
                            .build())
                    .toCompletableFuture()
                    .join());
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
        resolveWithContext(List.of(flag1), "foo", Struct.newBuilder().build(), true);

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
        resolveWithContext(List.of(flag1), "foo", Struct.newBuilder().build(), true);
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
      resolveWithContext(List.of(flag1), "foo", Struct.newBuilder().build(), true);
    } catch (Exception e) {
      assertThat(e).isInstanceOf(RuntimeException.class);
      assertThat(e.getMessage()).contains("Max retries exceeded for missing materializations: 3");
    }
  }

  @Test
  public void testTooLongKey() {
    assertThatExceptionOfType(RuntimeException.class)
        .isThrownBy(
            () ->
                resolveWithContext(
                    List.of(flag1), "a".repeat(101), Struct.newBuilder().build(), false))
        .withMessageContaining("Targeting key is too larger, max 100 characters.");
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
    final var builder = com.spotify.confidence.flags.admin.v1.ResolverState.newBuilder();
    builder.addAllFlags(flagsMap.values());
    builder.addAllSegmentsNoBitsets(segments.values());
    // All-one bitset for each segment
    segments
        .keySet()
        .forEach(
            name ->
                builder.addBitsets(
                    com.spotify.confidence.flags.admin.v1.ResolverState.PackedBitset.newBuilder()
                        .setSegment(name)
                        .setFullBitset(true)
                        .build()));
    builder.addClients(Client.newBuilder().setName(clientName).build());
    builder.addClientCredentials(
        ClientCredential.newBuilder().setName(credentialName).setClientSecret(secret).build());
    return builder.build().toByteArray();
  }

  private ResolveFlagsResponse resolveWithContext(
      List<String> flags, String username, Struct struct, boolean apply, String secret) {
    return resolverApi
        .resolveWithSticky(
            ResolveWithStickyRequest.newBuilder()
                .setFailFastOnSticky(false)
                .setResolveRequest(
                    ResolveFlagsRequest.newBuilder()
                        .addAllFlags(flags)
                        .setClientSecret(secret)
                        .setEvaluationContext(
                            Structs.of(
                                "targeting_key", Values.of(username), "bar", Values.of(struct)))
                        .setApply(apply))
                .build())
        .toCompletableFuture()
        .join();
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
        ResolveWithStickyRequest.newBuilder()
            .setResolveRequest(builder)
            .setFailFastOnSticky(false)
            .build();
    return resolverApi.resolveWithSticky(request).toCompletableFuture().join();
  }

  private ResolveFlagsResponse resolveWithContext(
      List<String> flags, String username, Struct struct, boolean apply) {
    return resolveWithContext(flags, username, struct, apply, secret.getSecret());
  }
}
