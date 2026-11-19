package com.spotify.confidence.sdk;

import com.google.protobuf.Struct;
import com.google.protobuf.Value;
import com.spotify.confidence.flags.types.v1.FlagSchema;
import dev.openfeature.sdk.EvaluationContext;
import dev.openfeature.sdk.exceptions.TypeMismatchError;
import java.time.temporal.ChronoUnit;
import java.util.ArrayList;
import java.util.List;

/**
 * Converts between OpenFeature values and the resolver's protobuf representation, with a type-check
 * on every decoded value — including values under structures, where the wire schema cannot express
 * the expected type.
 *
 * <p>Defaults carry a schema so the module can merge them into nulls: booleans, strings, integers,
 * doubles and structures (fields must themselves have schemas); lists and instants have no protobuf
 * schema representation and are rejected as defaults.
 */
final class OpenFeatureValueMapper {

  static final String TARGETING_KEY = "targeting_key";
  static final String SKIP_APPLY = "_confidence_skip_apply";

  private OpenFeatureValueMapper() {}

  /** The evaluation context as a protobuf struct: {@code targeting_key} plus the attributes. */
  static Struct toProtoContext(EvaluationContext ctx) {
    Struct.Builder struct = Struct.newBuilder();
    String targetingKey = ctx == null ? null : ctx.getTargetingKey();
    if (targetingKey != null && !targetingKey.isEmpty()) {
      struct.putFields(TARGETING_KEY, Value.newBuilder().setStringValue(targetingKey).build());
    }
    if (ctx != null) {
      ctx.asMap()
          .forEach(
              (key, v) -> {
                if (TARGETING_KEY.equals(key) || SKIP_APPLY.equals(key)) {
                  return;
                }
                try {
                  struct.putFields(key, toProtoValue(key, v));
                } catch (IllegalArgumentException e) {
                  throw new IllegalArgumentException(
                      "evaluation context field " + quote(key) + ": " + e.getMessage(), e);
                }
              });
    }
    return struct.build();
  }

  /** True when the context sets {@code _confidence_skip_apply} to true. */
  static boolean skipApply(EvaluationContext ctx) {
    if (ctx == null) {
      return false;
    }
    dev.openfeature.sdk.Value v = ctx.getValue(SKIP_APPLY);
    return v != null && v.isBoolean() && Boolean.TRUE.equals(v.asBoolean());
  }

  private static Value toProtoValue(String key, dev.openfeature.sdk.Value v) {
    if (v == null || v.isNull()) {
      return Value.newBuilder().setNullValue(com.google.protobuf.NullValue.NULL_VALUE).build();
    }
    if (v.isBoolean()) {
      return Value.newBuilder().setBoolValue(v.asBoolean()).build();
    }
    if (v.isString()) {
      return Value.newBuilder().setStringValue(v.asString()).build();
    }
    if (v.isNumber()) {
      return Value.newBuilder().setNumberValue(requireFinite(key, v.asDouble())).build();
    }
    if (v.isInstant()) {
      // Struct values can only hold numbers; serialize as RFC 3339, as the Go provider does.
      return Value.newBuilder()
          .setStringValue(v.asInstant().truncatedTo(ChronoUnit.MILLIS).toString())
          .build();
    }
    if (v.isStructure()) {
      Struct.Builder struct = Struct.newBuilder();
      v.asStructure()
          .asMap()
          .forEach((k, field) -> struct.putFields(k, toProtoValue(key + "." + k, field)));
      return Value.newBuilder().setStructValue(struct).build();
    }
    if (v.isList()) {
      List<Value> values = new ArrayList<>();
      List<dev.openfeature.sdk.Value> list = v.asList();
      for (int i = 0; i < list.size(); i++) {
        values.add(toProtoValue(key + "[" + i + "]", list.get(i)));
      }
      return Value.newBuilder()
          .setListValue(com.google.protobuf.ListValue.newBuilder().addAllValues(values))
          .build();
    }
    throw new IllegalArgumentException("unsupported value type " + v.getClass().getSimpleName());
  }

  private static double requireFinite(String key, double d) {
    if (!Double.isFinite(d)) {
      throw new IllegalArgumentException(quote(key) + " is not finite");
    }
    return d;
  }

  // --------------------------------------------------------------- default encoding

  /** A default encoded for the module: the value plus the schema the module should merge into. */
  record EncodedDefault(Value value, FlagSchema schema) {}

  /**
   * Encodes a default value and its schema. Structures are supported (each field must have a
   * schema); lists and instants are rejected, as they have no schema representation.
   */
  static EncodedDefault encodeDefault(dev.openfeature.sdk.Value def) {
    if (def == null || def.isNull()) {
      return new EncodedDefault(nullValue(), null);
    }
    if (def.isBoolean()) {
      return new EncodedDefault(
          Value.newBuilder().setBoolValue(def.asBoolean()).build(), boolSchema());
    }
    if (def.isString()) {
      return new EncodedDefault(
          Value.newBuilder().setStringValue(def.asString()).build(), stringSchema());
    }
    if (isIntegerValue(def)) {
      return new EncodedDefault(
          Value.newBuilder().setNumberValue(def.asInteger()).build(), intSchema());
    }
    if (def.isNumber()) {
      return new EncodedDefault(
          Value.newBuilder().setNumberValue(requireFinite("default", def.asDouble())).build(),
          doubleSchema());
    }
    if (def.isStructure()) {
      Struct.Builder struct = Struct.newBuilder();
      FlagSchema.StructFlagSchema.Builder schema = FlagSchema.StructFlagSchema.newBuilder();
      def.asStructure()
          .asMap()
          .forEach(
              (name, field) -> {
                EncodedDefault encoded = encodeDefault(field);
                if (encoded.schema() == null) {
                  throw mismatch(
                      "field " + quote(name) + " of the default has no schema representation");
                }
                struct.putFields(name, encoded.value());
                schema.putSchema(name, encoded.schema());
              });
      return new EncodedDefault(
          Value.newBuilder().setStructValue(struct).build(),
          FlagSchema.newBuilder().setStructSchema(schema).build());
    }
    throw mismatch("unsupported default type " + describe(def));
  }

  /** The SDK's Value does not distinguish int from double numbers; the boxed object does. */
  private static boolean isIntegerValue(dev.openfeature.sdk.Value v) {
    return v.isNumber() && v.asObject() instanceof Integer;
  }

  private static FlagSchema boolSchema() {
    return FlagSchema.newBuilder().setBoolSchema(FlagSchema.BoolFlagSchema.newBuilder()).build();
  }

  private static FlagSchema stringSchema() {
    return FlagSchema.newBuilder()
        .setStringSchema(FlagSchema.StringFlagSchema.newBuilder())
        .build();
  }

  static FlagSchema intSchema() {
    return FlagSchema.newBuilder().setIntSchema(FlagSchema.IntFlagSchema.newBuilder()).build();
  }

  private static FlagSchema doubleSchema() {
    return FlagSchema.newBuilder()
        .setDoubleSchema(FlagSchema.DoubleFlagSchema.newBuilder())
        .build();
  }

  // ----------------------------------------------------------------- decoding

  /**
   * Decodes a resolved value against the shape of the caller's default. Every value is type checked
   * — including the elements under structures, where the module cannot enforce the expected type. A
   * null resolved value yields the default.
   */
  static dev.openfeature.sdk.Value decode(Value value, dev.openfeature.sdk.Value def) {
    if (value == null || value.getKindCase() == Value.KindCase.NULL_VALUE) {
      return def == null ? new dev.openfeature.sdk.Value() : def;
    }
    if (def != null && def.isBoolean()) {
      return new dev.openfeature.sdk.Value(
          requireKind(value, Value.KindCase.BOOL_VALUE).getBoolValue());
    }
    if (def != null && def.isString()) {
      return new dev.openfeature.sdk.Value(
          requireKind(value, Value.KindCase.STRING_VALUE).getStringValue());
    }
    if (def != null && isIntegerValue(def)) {
      return new dev.openfeature.sdk.Value(decodeInt(value));
    }
    if (def != null && def.isNumber()) {
      return new dev.openfeature.sdk.Value(
          requireKind(value, Value.KindCase.NUMBER_VALUE).getNumberValue());
    }
    if (def != null && def.isStructure()) {
      Value struct = requireKind(value, Value.KindCase.STRUCT_VALUE);
      var template = def.asStructure().asMap();
      var out = new java.util.HashMap<String, dev.openfeature.sdk.Value>();
      struct
          .getStructValue()
          .getFieldsMap()
          .forEach(
              (k, v) -> {
                dev.openfeature.sdk.Value fieldTemplate = template.get(k);
                out.put(k, fieldTemplate == null ? natural(v) : decode(v, fieldTemplate));
              });
      // The module merges defaults into nulls; keep the caller's default for absent fields.
      template.forEach(out::putIfAbsent);
      return new dev.openfeature.sdk.Value(new dev.openfeature.sdk.MutableStructure(out));
    }
    return natural(value);
  }

  private static int decodeInt(Value value) {
    Value number = requireKind(value, Value.KindCase.NUMBER_VALUE);
    double d = number.getNumberValue();
    if (d != Math.rint(d) || !Double.isFinite(d)) {
      throw mismatch("resolved value " + d + " is not a whole number");
    }
    if (d < Integer.MIN_VALUE || d > Integer.MAX_VALUE) {
      throw mismatch("resolved value " + d + " does not fit an int");
    }
    return (int) d;
  }

  /** Converts without a template; numbers become doubles, as in any JSON value. */
  private static dev.openfeature.sdk.Value natural(Value value) {
    switch (value.getKindCase()) {
      case BOOL_VALUE -> {
        return new dev.openfeature.sdk.Value(value.getBoolValue());
      }
      case STRING_VALUE -> {
        return new dev.openfeature.sdk.Value(value.getStringValue());
      }
      case NUMBER_VALUE -> {
        return new dev.openfeature.sdk.Value(value.getNumberValue());
      }
      case NULL_VALUE -> {
        return new dev.openfeature.sdk.Value();
      }
      case STRUCT_VALUE -> {
        var fields = new java.util.HashMap<String, dev.openfeature.sdk.Value>();
        value.getStructValue().getFieldsMap().forEach((k, v) -> fields.put(k, natural(v)));
        return new dev.openfeature.sdk.Value(new dev.openfeature.sdk.MutableStructure(fields));
      }
      case LIST_VALUE -> {
        List<dev.openfeature.sdk.Value> values = new ArrayList<>();
        for (Value v : value.getListValue().getValuesList()) {
          values.add(natural(v));
        }
        return new dev.openfeature.sdk.Value(values);
      }
      default -> throw mismatch("resolved value has kind " + kindName(value));
    }
  }

  private static Value requireKind(Value value, Value.KindCase kind) {
    if (value.getKindCase() != kind) {
      throw mismatch(
          "resolved value is "
              + kindName(value)
              + ", expected "
              + kind.name().replace("_VALUE", "").toLowerCase(java.util.Locale.ROOT));
    }
    return value;
  }

  private static String kindName(Value value) {
    return value.getKindCase().name().replace("_VALUE", "").toLowerCase(java.util.Locale.ROOT);
  }

  private static Value nullValue() {
    return Value.newBuilder().setNullValue(com.google.protobuf.NullValue.NULL_VALUE).build();
  }

  private static TypeMismatchError mismatch(String message) {
    return new TypeMismatchError(message);
  }

  private static String describe(dev.openfeature.sdk.Value v) {
    if (v.isList()) {
      return "list";
    }
    if (v.isInstant()) {
      return "instant";
    }
    return v.getClass().getSimpleName();
  }

  private static String quote(String s) {
    return "\"" + s + "\"";
  }
}
