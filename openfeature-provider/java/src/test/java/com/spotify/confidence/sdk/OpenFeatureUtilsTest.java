package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;

import dev.openfeature.sdk.MutableContext;
import org.junit.jupiter.api.Test;

class OpenFeatureUtilsTest {

  @Test
  void isSkipApply_returnsTrueWhenSet() {
    final var ctx = new MutableContext("user-1");
    ctx.add("_confidence_skip_apply", true);
    assertThat(OpenFeatureUtils.isSkipApply(ctx)).isTrue();
  }

  @Test
  void isSkipApply_returnsFalseWhenNotSet() {
    final var ctx = new MutableContext("user-1");
    assertThat(OpenFeatureUtils.isSkipApply(ctx)).isFalse();
  }

  @Test
  void isSkipApply_returnsFalseWhenSetToFalse() {
    final var ctx = new MutableContext("user-1");
    ctx.add("_confidence_skip_apply", false);
    assertThat(OpenFeatureUtils.isSkipApply(ctx)).isFalse();
  }

  @Test
  void convertToProto_stripsSkipApplyKey() {
    final var ctx = new MutableContext("user-1");
    ctx.add("_confidence_skip_apply", true);
    ctx.add("country", "SE");

    final var proto = OpenFeatureUtils.convertToProto(ctx);

    assertThat(proto.getFieldsMap()).doesNotContainKey("_confidence_skip_apply");
    assertThat(proto.getFieldsMap()).containsKey("country");
    assertThat(proto.getFieldsMap()).containsKey("targeting_key");
  }
}
