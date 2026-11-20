package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyBoolean;
import static org.mockito.ArgumentMatchers.anyList;
import static org.mockito.ArgumentMatchers.argThat;
import static org.mockito.ArgumentMatchers.eq;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import com.spotify.confidence.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.flags.resolver.v1.ResolveFlagsResponse;
import com.spotify.confidence.flags.resolver.v1.ResolveReason;
import com.spotify.confidence.flags.resolver.v1.ResolvedFlag;
import dev.openfeature.sdk.EvaluationContext;
import dev.openfeature.sdk.Value;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;

/**
 * The JSON HTTP service, as the old stack's FlagResolverServiceTest; that file is the reference for
 * the status codes and the context extraction. The provider is mocked, so these tests only cover
 * the service layer.
 */
class FlagResolverServiceTest {

  /** The old request adapter, kept verbatim. */
  record TestRequest(String method, byte[] body, Map<String, List<String>> headers)
      implements ConfidenceHttpRequest {}

  static TestRequest post(String body) {
    return request("POST", "application/json", body);
  }

  static TestRequest request(String method, String contentType, String body) {
    Map<String, List<String>> headers = new HashMap<>();
    if (contentType != null) {
      headers.put("Content-Type", List.of(contentType));
    }
    return new TestRequest(method, body.getBytes(StandardCharsets.UTF_8), headers);
  }

  static TestRequest withHeader(TestRequest request, String name, String value) {
    Map<String, List<String>> headers = new HashMap<>(request.headers());
    headers.put(name, List.of(value));
    return new TestRequest(request.method(), request.body(), headers);
  }

  private OpenFeatureLocalResolveProvider provider;
  private FlagResolverService<TestRequest> service;

  @BeforeEach
  void setUp() {
    provider = mock(OpenFeatureLocalResolveProvider.class);
    when(provider.resolve(any(), anyList(), anyBoolean()))
        .thenReturn(CompletableFuture.completedFuture(cannedResponse()));
    service = new FlagResolverService<>(provider);
  }

  private static ResolveFlagsResponse cannedResponse() {
    return ResolveFlagsResponse.newBuilder()
        .setResolveId("resolve-id-1")
        .addResolvedFlags(
            ResolvedFlag.newBuilder()
                .setFlag("flags/flag-1")
                .setVariant("flags/flag-1/variants/on")
                .setReason(ResolveReason.RESOLVE_REASON_MATCH))
        .build();
  }

  // ----------------------------------------------------------------- resolve

  @Test
  void shouldReturn405ForNonPostMethod() {
    assertEquals(
        405,
        service
            .handleResolve(request("GET", "application/json", "{}"))
            .toCompletableFuture()
            .join()
            .statusCode());
  }

  @Test
  void shouldReturn415ForNonJsonContentType() {
    assertEquals(
        415,
        service
            .handleResolve(request("POST", "text/plain", "{}"))
            .toCompletableFuture()
            .join()
            .statusCode());
  }

  @Test
  void shouldReturn415ForMissingContentType() {
    assertEquals(
        415,
        service
            .handleResolve(request("POST", null, "{}"))
            .toCompletableFuture()
            .join()
            .statusCode());
  }

  @Test
  void shouldReturn400ForInvalidJson() {
    assertEquals(
        400, service.handleResolve(post("not json")).toCompletableFuture().join().statusCode());
    assertEquals(400, service.handleResolve(post("")).toCompletableFuture().join().statusCode());
  }

  @Test
  void shouldAcceptLowercaseAndMixedCaseMethodAndContentType() {
    assertEquals(
        200,
        service
            .handleResolve(request("post", "application/json", "{\"flags\":[\"flag-1\"]}"))
            .toCompletableFuture()
            .join()
            .statusCode());
    assertEquals(
        200,
        service
            .handleResolve(request("PoSt", "APPLICATION/JSON", "{\"flags\":[\"flag-1\"]}"))
            .toCompletableFuture()
            .join()
            .statusCode());
  }

  @Test
  void shouldResolveFlags() {
    ConfidenceHttpResponse response =
        service
            .handleResolve(
                post(
                    "{\"flags\":[\"flag1\",\"flag2\"],"
                        + "\"evaluationContext\":{\"targeting_key\":\"user123\",\"country\":\"US\"},"
                        + "\"apply\":false}"))
            .toCompletableFuture()
            .join();

    assertEquals(200, response.statusCode());
    String body = new String(response.body(), StandardCharsets.UTF_8);
    assertTrue(body.contains("flags/flag-1"), body);
    assertTrue(body.contains("RESOLVE_REASON_MATCH"), body);
    assertTrue(body.contains("resolveId"), body);

    ArgumentCaptor<EvaluationContext> ctx = ArgumentCaptor.forClass(EvaluationContext.class);
    verify(provider)
        .resolve(
            ctx.capture(), argThat(flags -> flags.equals(List.of("flag1", "flag2"))), eq(false));
    assertEquals("user123", ctx.getValue().getTargetingKey());
    assertEquals("US", ctx.getValue().getValue("country").asString());
  }

  @Test
  void shouldHandleFlagsWithPrefix() {
    service.handleResolve(post("{\"flags\":[\"flags/my-flag\"]}")).toCompletableFuture().join();
    verify(provider)
        .resolve(any(), argThat(flags -> flags.equals(List.of("flags/my-flag"))), anyBoolean());
  }

  @Test
  void shouldExtractValuesFromContext() {
    service
        .handleResolve(
            post(
                "{\"flags\":[\"flag-1\"],\"evaluationContext\":{"
                    + "\"targeting_key\":\"user-abc-123\",\"country\":\"SE\",\"version\":\"1.2.3\","
                    + "\"age\":25.0,\"score\":99.5,\"premium\":true,"
                    + "\"tags\":[\"a\",\"b\",\"c\"],"
                    + "\"user\":{\"id\":\"u123\",\"name\":\"Test User\"}}}"))
        .toCompletableFuture()
        .join();

    ArgumentCaptor<EvaluationContext> ctx = ArgumentCaptor.forClass(EvaluationContext.class);
    verify(provider).resolve(ctx.capture(), anyList(), anyBoolean());
    EvaluationContext context = ctx.getValue();
    assertEquals("user-abc-123", context.getTargetingKey());
    assertEquals("SE", context.getValue("country").asString());
    assertEquals("1.2.3", context.getValue("version").asString());
    assertEquals(25.0, context.getValue("age").asDouble());
    assertEquals(99.5, context.getValue("score").asDouble());
    assertEquals(Boolean.TRUE, context.getValue("premium").asBoolean());
    assertEquals(
        List.of("a", "b", "c"),
        context.getValue("tags").asList().stream().map(Value::asString).toList());
    assertEquals("u123", context.getValue("user").asStructure().getValue("id").asString());
    assertEquals("Test User", context.getValue("user").asStructure().getValue("name").asString());
  }

  @Test
  void shouldIgnoreUnknownFieldsInRequest() {
    // clientSecret, sdk and unknown fields are the provider's business, not the request's.
    ConfidenceHttpResponse response =
        service
            .handleResolve(
                post(
                    "{\"flags\":[\"flag-1\"],\"clientSecret\":\"nope\","
                        + "\"sdk\":{\"id\":1},\"unknown\":{\"nested\":true}}"))
            .toCompletableFuture()
            .join();
    assertEquals(200, response.statusCode());
  }

  @Test
  void shouldHandleEmptyFlagsList() {
    assertEquals(
        200,
        service.handleResolve(post("{\"flags\":[]}")).toCompletableFuture().join().statusCode());
    verify(provider).resolve(any(), argThat(flags -> flags.isEmpty()), anyBoolean());
  }

  @Test
  void shouldReturn500WhenProviderThrowsException() {
    when(provider.resolve(any(), anyList(), anyBoolean()))
        .thenReturn(CompletableFuture.failedFuture(new RuntimeException("boom")));
    assertEquals(
        500,
        service
            .handleResolve(post("{\"flags\":[\"flag-1\"]}"))
            .toCompletableFuture()
            .join()
            .statusCode());
  }

  // ------------------------------------------------------------------- apply

  @Test
  void applyShouldReturn405ForNonPostMethod() {
    assertEquals(
        405,
        service
            .handleApply(request("GET", "application/json", "{}"))
            .toCompletableFuture()
            .join()
            .statusCode());
  }

  @Test
  void applyShouldReturn415ForNonJsonContentType() {
    assertEquals(
        415,
        service
            .handleApply(request("POST", "text/plain", "{}"))
            .toCompletableFuture()
            .join()
            .statusCode());
  }

  @Test
  void applyShouldReturn400ForInvalidJson() {
    assertEquals(
        400, service.handleApply(post("not json")).toCompletableFuture().join().statusCode());
    assertEquals(400, service.handleApply(post("")).toCompletableFuture().join().statusCode());
  }

  @Test
  void shouldApplyFlags() {
    ConfidenceHttpResponse response =
        service
            .handleApply(
                post(
                    "{\"flags\":[{\"flag\":\"flags/my-flag\","
                        + "\"applyTime\":\"2025-01-01T00:00:00Z\"}],"
                        + "\"resolveToken\":\"dG9rZW4=\",\"sendTime\":\"2025-01-01T00:00:00Z\"}"))
            .toCompletableFuture()
            .join();

    assertEquals(200, response.statusCode());
    assertEquals("{}", new String(response.body(), StandardCharsets.UTF_8));

    ArgumentCaptor<ApplyFlagsRequest> captor = ArgumentCaptor.forClass(ApplyFlagsRequest.class);
    verify(provider).applyFlags(captor.capture());
    assertEquals(1, captor.getValue().getFlagsCount());
    assertEquals("flags/my-flag", captor.getValue().getFlags(0).getFlag());
  }

  @Test
  void shouldApplyMultipleFlags() {
    service
        .handleApply(
            post(
                "{\"flags\":[{\"flag\":\"flags/a\","
                    + "\"applyTime\":\"2025-01-01T00:00:00Z\"},"
                    + "{\"flag\":\"flags/b\",\"applyTime\":\"2025-01-01T00:00:00Z\"}],"
                    + "\"resolveToken\":\"dG9rZW4=\",\"sendTime\":\"2025-01-01T00:00:00Z\"}"))
        .toCompletableFuture()
        .join();

    ArgumentCaptor<ApplyFlagsRequest> captor = ArgumentCaptor.forClass(ApplyFlagsRequest.class);
    verify(provider).applyFlags(captor.capture());
    assertEquals(2, captor.getValue().getFlagsCount());
  }

  @Test
  void shouldHandleEmptyFlagsListOnApply() {
    ConfidenceHttpResponse response =
        service
            .handleApply(
                post(
                    "{\"flags\":[],\"resolveToken\":\"dG9rZW4=\","
                        + "\"sendTime\":\"2025-01-01T00:00:00Z\"}"))
            .toCompletableFuture()
            .join();
    assertEquals(200, response.statusCode());
    verify(provider).applyFlags(any());
  }

  @Test
  void applyShouldReturn500WhenProviderThrowsException() {
    doThrow(new RuntimeException("boom")).when(provider).applyFlags(any());
    assertEquals(
        500,
        service
            .handleApply(
                post(
                    "{\"flags\":[],\"resolveToken\":\"dG9rZW4=\","
                        + "\"sendTime\":\"2025-01-01T00:00:00Z\"}"))
            .toCompletableFuture()
            .join()
            .statusCode());
  }

  // --------------------------------------------------------------- decorators

  @Test
  void shouldApplyContextDecorator() {
    FlagResolverService<TestRequest> decorated =
        new FlagResolverService<>(
            provider,
            ContextDecorator.sync(
                (ctx, req) -> {
                  String userId = req.headers().getOrDefault("X-User-Id", List.of()).get(0);
                  dev.openfeature.sdk.MutableContext out =
                      new dev.openfeature.sdk.MutableContext(ctx.getTargetingKey());
                  out.add("decorated_user_id", userId);
                  return out;
                }));

    decorated
        .handleResolve(withHeader(post("{\"flags\":[\"flag-1\"]}"), "X-User-Id", "decorated-123"))
        .toCompletableFuture()
        .join();

    ArgumentCaptor<EvaluationContext> ctx = ArgumentCaptor.forClass(EvaluationContext.class);
    verify(provider).resolve(ctx.capture(), anyList(), anyBoolean());
    assertEquals("decorated-123", ctx.getValue().getValue("decorated_user_id").asString());
  }

  @Test
  void shouldCombineRequestContextWithDecorator() {
    FlagResolverService<TestRequest> decorated =
        new FlagResolverService<>(
            provider,
            ContextDecorator.sync(
                (ctx, req) -> {
                  dev.openfeature.sdk.MutableContext out =
                      new dev.openfeature.sdk.MutableContext(ctx.getTargetingKey());
                  out.add("source", "backend_proxy");
                  ctx.asMap().forEach((k, v) -> copyValue(out, k, v));
                  return out;
                }));

    decorated
        .handleResolve(
            post(
                "{\"flags\":[\"flag-1\"],"
                    + "\"evaluationContext\":{\"targeting_key\":\"user-1\",\"country\":\"SE\"}}"))
        .toCompletableFuture()
        .join();

    ArgumentCaptor<EvaluationContext> ctx = ArgumentCaptor.forClass(EvaluationContext.class);
    verify(provider).resolve(ctx.capture(), anyList(), anyBoolean());
    assertEquals("SE", ctx.getValue().getValue("country").asString());
    assertEquals("backend_proxy", ctx.getValue().getValue("source").asString());
  }

  private static void copyValue(dev.openfeature.sdk.MutableContext out, String key, Value v) {
    if (v.isString()) {
      out.add(key, v.asString());
    } else if (v.isNumber()) {
      if (v.asObject() instanceof Integer i) {
        out.add(key, i);
      } else {
        out.add(key, v.asDouble());
      }
    } else if (v.isBoolean()) {
      out.add(key, v.asBoolean());
    } else if (v.isStructure()) {
      out.add(key, v.asStructure());
    } else if (v.isList()) {
      out.add(key, v.asList());
    }
  }
}
