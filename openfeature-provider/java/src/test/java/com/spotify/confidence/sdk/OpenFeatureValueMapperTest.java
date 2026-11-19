package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

import com.google.protobuf.Value;
import dev.openfeature.sdk.MutableStructure;
import dev.openfeature.sdk.exceptions.TypeMismatchError;
import java.util.Map;
import org.junit.jupiter.api.Test;

/**
 * The value codec's checks, as the Go provider's value_test.go; that file is the reference for the
 * assertions.
 */
class OpenFeatureValueMapperTest {

  private static Value number(double d) {
    return Value.newBuilder().setNumberValue(d).build();
  }

  private static Value string(String s) {
    return Value.newBuilder().setStringValue(s).build();
  }

  @Test
  void decodeIntChecks() {
    // Whole numbers in range decode.
    assertEquals(
        3,
        OpenFeatureValueMapper.decode(number(3.0), new dev.openfeature.sdk.Value(0)).asInteger());
    assertEquals(
        -3,
        OpenFeatureValueMapper.decode(number(-3.0), new dev.openfeature.sdk.Value(0)).asInteger());
    assertEquals(
        Integer.MAX_VALUE,
        OpenFeatureValueMapper.decode(number(Integer.MAX_VALUE), new dev.openfeature.sdk.Value(0))
            .asInteger());

    // Fractions and out-of-range values are mismatches, not truncations.
    assertThrows(
        TypeMismatchError.class,
        () -> OpenFeatureValueMapper.decode(number(3.5), new dev.openfeature.sdk.Value(0)));
    assertThrows(
        TypeMismatchError.class,
        () -> OpenFeatureValueMapper.decode(number(1e10), new dev.openfeature.sdk.Value(0)));
    assertThrows(
        TypeMismatchError.class,
        () -> OpenFeatureValueMapper.decode(string("3"), new dev.openfeature.sdk.Value(0)));
  }

  @Test
  void encodeDefaultSchema() {
    OpenFeatureValueMapper.EncodedDefault anInt =
        OpenFeatureValueMapper.encodeDefault(new dev.openfeature.sdk.Value(7));
    OpenFeatureValueMapper.EncodedDefault aDouble =
        OpenFeatureValueMapper.encodeDefault(new dev.openfeature.sdk.Value(7.5));

    assertEquals(
        com.spotify.confidence.flags.types.v1.FlagSchema.SchemaTypeCase.INT_SCHEMA,
        anInt.schema().getSchemaTypeCase());
    assertEquals(
        com.spotify.confidence.flags.types.v1.FlagSchema.SchemaTypeCase.DOUBLE_SCHEMA,
        aDouble.schema().getSchemaTypeCase());

    // A structure default carries per-field schemas: ints stay ints, doubles stay doubles.
    MutableStructure nested =
        new MutableStructure(
            Map.of(
                "count", new dev.openfeature.sdk.Value(1),
                "ratio", new dev.openfeature.sdk.Value(1.5)));
    OpenFeatureValueMapper.EncodedDefault struct =
        OpenFeatureValueMapper.encodeDefault(new dev.openfeature.sdk.Value(nested));
    com.spotify.confidence.flags.types.v1.FlagSchema.StructFlagSchema schema =
        struct.schema().getStructSchema();
    assertEquals(
        com.spotify.confidence.flags.types.v1.FlagSchema.SchemaTypeCase.INT_SCHEMA,
        schema.getSchemaOrThrow("count").getSchemaTypeCase());
    assertEquals(
        com.spotify.confidence.flags.types.v1.FlagSchema.SchemaTypeCase.DOUBLE_SCHEMA,
        schema.getSchemaOrThrow("ratio").getSchemaTypeCase());

    // Lists and instants have no schema representation and are rejected.
    assertThrows(
        TypeMismatchError.class,
        () ->
            OpenFeatureValueMapper.encodeDefault(
                new dev.openfeature.sdk.Value(
                    java.util.List.of(new dev.openfeature.sdk.Value(1)))));
  }

  @Test
  void decodeChecksKindsUnderMaps() {
    // A resolved string where the default's field is an int is a mismatch, even under a map.
    MutableStructure template = new MutableStructure(Map.of("a", new dev.openfeature.sdk.Value(0)));
    Value resolved =
        Value.newBuilder()
            .setStructValue(
                com.google.protobuf.Struct.newBuilder().putFields("a", string("str")).build())
            .build();
    assertThrows(
        TypeMismatchError.class,
        () -> OpenFeatureValueMapper.decode(resolved, new dev.openfeature.sdk.Value(template)));

    // A double where the default's field is a bool, under a nested structure.
    MutableStructure boolTemplate =
        new MutableStructure(Map.of("b", new dev.openfeature.sdk.Value(true)));
    Value boolResolved =
        Value.newBuilder()
            .setStructValue(
                com.google.protobuf.Struct.newBuilder().putFields("b", number(1.0)).build())
            .build();
    assertThrows(
        TypeMismatchError.class,
        () ->
            OpenFeatureValueMapper.decode(
                boolResolved, new dev.openfeature.sdk.Value(boolTemplate)));

    // Nulls decode to the default.
    dev.openfeature.sdk.Value def = new dev.openfeature.sdk.Value(5);
    assertEquals(
        def,
        OpenFeatureValueMapper.decode(
            Value.newBuilder().setNullValue(com.google.protobuf.NullValue.NULL_VALUE).build(),
            def));
  }

  @Test
  void absentStructFieldsAreZeroFilledAsInGo() {
    // A field present in the default but absent from the resolved value is the type's zero,
    // not the default's value (Go's decodeTo zero-fills; the module merges every schema'd
    // field, so absence means the flag has no such field).
    MutableStructure template =
        new MutableStructure(
            Map.of(
                "count", new dev.openfeature.sdk.Value(7),
                "ratio", new dev.openfeature.sdk.Value(0.5),
                "label", new dev.openfeature.sdk.Value("default"),
                "flag", new dev.openfeature.sdk.Value(true)));
    Value resolved =
        Value.newBuilder()
            .setStructValue(
                com.google.protobuf.Struct.newBuilder().putFields("count", number(3.0)).build())
            .build();

    dev.openfeature.sdk.Value out =
        OpenFeatureValueMapper.decode(resolved, new dev.openfeature.sdk.Value(template));

    assertEquals(3, out.asStructure().getValue("count").asInteger());
    assertEquals(0.0, out.asStructure().getValue("ratio").asDouble());
    assertEquals("", out.asStructure().getValue("label").asString());
    assertEquals(false, out.asStructure().getValue("flag").asBoolean());
  }
}
