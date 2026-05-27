package com.spotify.confidence.sdk;

import com.google.common.annotations.Beta;
import com.google.protobuf.Struct;
import dev.openfeature.sdk.EvaluationContext;
import dev.openfeature.sdk.Structure;
import dev.openfeature.sdk.Value;
import dev.openfeature.sdk.exceptions.TypeMismatchError;
import java.util.List;
import org.slf4j.Logger;

@Beta
class OpenFeatureUtils {

  static final String TARGETING_KEY = "targeting_key";
  static final String SKIP_APPLY_KEY = "_confidence_skip_apply";
  private static final Logger log = org.slf4j.LoggerFactory.getLogger(OpenFeatureUtils.class);

  static boolean isSkipApply(EvaluationContext evaluationContext) {
    final Value value = evaluationContext.getValue(SKIP_APPLY_KEY);
    return value != null && value.isBoolean() && value.asBoolean();
  }

  /*
  OpenFeature Evaluation Context -> Proto
   */
  static Struct convertToProto(EvaluationContext evaluationContext) {
    final Struct.Builder protoEvaluationContext = Struct.newBuilder();
    evaluationContext
        .asMap()
        .forEach(
            (mapKey, mapValue) -> {
              if (!SKIP_APPLY_KEY.equals(mapKey)) {
                protoEvaluationContext.putFields(mapKey, OpenFeatureTypeMapper.from(mapValue));
              }
            });
    // add targeting key as a regular value to proto struct
    if (evaluationContext.getTargetingKey() != null
        && !evaluationContext.getTargetingKey().isEmpty()) {
      protoEvaluationContext.putFields(
          TARGETING_KEY,
          com.google.protobuf.Value.newBuilder()
              .setStringValue(evaluationContext.getTargetingKey())
              .build());
    }
    return protoEvaluationContext.build();
  }

  /*
  OpenFeature "value for path"
   */
  static Value getValueForPath(List<String> path, Value fullValue) {
    Value value = fullValue;
    for (String fieldName : path) {
      final Structure structure = value.asStructure();
      if (structure == null) {
        // value's inner object actually is no structure
        log.warn(
            "Illegal attempt to derive field '{}' on non-structure value '{}'", fieldName, value);
        throw new TypeMismatchError(
            String.format(
                "Illegal attempt to derive field '%s' on non-structure value '%s'",
                fieldName, value));
      }

      value = structure.getValue(fieldName);

      if (value == null) {
        // we know that null indicates absence of a proper value because intended nulls would be an
        // instance of type Value
        log.warn(
            "Illegal attempt to derive non-existing field '{}' on structure value '{}'",
            fieldName,
            structure);
        throw new TypeMismatchError(
            String.format(
                "Illegal attempt to derive non-existing field '%s' on structure value '%s'",
                fieldName, structure));
      }
    }

    return value;
  }
}
