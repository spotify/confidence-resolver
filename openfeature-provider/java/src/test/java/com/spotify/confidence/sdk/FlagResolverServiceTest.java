package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyBoolean;
import static org.mockito.ArgumentMatchers.anyList;
import static org.mockito.ArgumentMatchers.eq;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import com.google.protobuf.ByteString;
import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsResponse;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveReason;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolvedFlag;
import dev.openfeature.sdk.EvaluationContext;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;

class FlagResolverServiceTest {

  private OpenFeatureLocalResolveProvider mockProvider;
  private FlagResolverService service;

  @BeforeEach
  void setUp() {
    mockProvider = mock(OpenFeatureLocalResolveProvider.class);
    service = new FlagResolverService(mockProvider);
  }

  @Nested
  class HandleResolve {

    @Test
    void shouldReturn405ForNonPostMethod() {
      ConfidenceHttpRequest request = createRequest("GET", "{}");

      ConfidenceHttpResponse response = service.handleResolve(request);

      assertThat(response.getStatusCode()).isEqualTo(405);
    }

    @Test
    void shouldReturn400ForInvalidJson() {
      ConfidenceHttpRequest request = createRequest("POST", "not valid json {{{");

      ConfidenceHttpResponse response = service.handleResolve(request);

      assertThat(response.getStatusCode()).isEqualTo(400);
    }

    @Test
    void shouldResolveFlags() {
      String requestBody =
          """
          {
            "flags": ["flag1", "flag2"],
            "evaluationContext": {
              "targeting_key": "user123"
            },
            "apply": false
          }
          """;

      ResolveFlagsResponse mockResponse =
          ResolveFlagsResponse.newBuilder()
              .addResolvedFlags(
                  ResolvedFlag.newBuilder()
                      .setFlag("flags/flag1")
                      .setVariant("flags/flag1/variants/control")
                      .setReason(ResolveReason.RESOLVE_REASON_MATCH)
                      .build())
              .setResolveToken(ByteString.copyFromUtf8("test-token"))
              .setResolveId("resolve-123")
              .build();

      when(mockProvider.resolve(any(EvaluationContext.class), anyList(), eq(false)))
          .thenReturn(mockResponse);

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      ConfidenceHttpResponse response = service.handleResolve(request);

      assertThat(response.getStatusCode()).isEqualTo(200);
      assertThat(response.getHeaders().get("Content-Type")).isEqualTo("application/json");

      String body = readBody(response);
      assertThat(body).contains("flags/flag1");
      assertThat(body).contains("RESOLVE_REASON_MATCH");
      assertThat(body).contains("resolveId");

      verify(mockProvider)
          .resolve(any(EvaluationContext.class), eq(List.of("flag1", "flag2")), eq(false));
    }

    @Test
    void shouldResolveFlagsWithApplyTrue() {
      String requestBody =
          """
          {
            "flags": ["my-flag"],
            "evaluationContext": {},
            "apply": true
          }
          """;

      when(mockProvider.resolve(any(), anyList(), eq(true)))
          .thenReturn(ResolveFlagsResponse.getDefaultInstance());

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      ConfidenceHttpResponse response = service.handleResolve(request);

      assertThat(response.getStatusCode()).isEqualTo(200);
      verify(mockProvider).resolve(any(), eq(List.of("my-flag")), eq(true));
    }

    @Test
    void shouldHandleFlagsWithPrefix() {
      String requestBody =
          """
          {
            "flags": ["flags/already-prefixed"],
            "evaluationContext": {},
            "apply": false
          }
          """;

      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenReturn(ResolveFlagsResponse.getDefaultInstance());

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      service.handleResolve(request);

      verify(mockProvider).resolve(any(), eq(List.of("flags/already-prefixed")), eq(false));
    }

    @Test
    void shouldExtractTargetingKeyFromContext() {
      String requestBody =
          """
          {
            "flags": ["test-flag"],
            "evaluationContext": {
              "targeting_key": "user-abc-123"
            },
            "apply": false
          }
          """;

      AtomicReference<EvaluationContext> capturedContext = new AtomicReference<>();
      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenAnswer(
              invocation -> {
                capturedContext.set(invocation.getArgument(0));
                return ResolveFlagsResponse.getDefaultInstance();
              });

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      service.handleResolve(request);

      assertThat(capturedContext.get().getTargetingKey()).isEqualTo("user-abc-123");
    }

    @Test
    void shouldExtractStringValuesFromContext() {
      String requestBody =
          """
          {
            "flags": ["test-flag"],
            "evaluationContext": {
              "country": "US",
              "version": "1.2.3"
            },
            "apply": false
          }
          """;

      AtomicReference<EvaluationContext> capturedContext = new AtomicReference<>();
      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenAnswer(
              invocation -> {
                capturedContext.set(invocation.getArgument(0));
                return ResolveFlagsResponse.getDefaultInstance();
              });

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      service.handleResolve(request);

      assertThat(capturedContext.get().getValue("country").asString()).isEqualTo("US");
      assertThat(capturedContext.get().getValue("version").asString()).isEqualTo("1.2.3");
    }

    @Test
    void shouldExtractNumberValuesFromContext() {
      String requestBody =
          """
          {
            "flags": ["test-flag"],
            "evaluationContext": {
              "age": 25,
              "score": 99.5
            },
            "apply": false
          }
          """;

      AtomicReference<EvaluationContext> capturedContext = new AtomicReference<>();
      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenAnswer(
              invocation -> {
                capturedContext.set(invocation.getArgument(0));
                return ResolveFlagsResponse.getDefaultInstance();
              });

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      service.handleResolve(request);

      assertThat(capturedContext.get().getValue("age").asDouble()).isEqualTo(25.0);
      assertThat(capturedContext.get().getValue("score").asDouble()).isEqualTo(99.5);
    }

    @Test
    void shouldExtractBooleanValuesFromContext() {
      String requestBody =
          """
          {
            "flags": ["test-flag"],
            "evaluationContext": {
              "premium": true,
              "trial": false
            },
            "apply": false
          }
          """;

      AtomicReference<EvaluationContext> capturedContext = new AtomicReference<>();
      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenAnswer(
              invocation -> {
                capturedContext.set(invocation.getArgument(0));
                return ResolveFlagsResponse.getDefaultInstance();
              });

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      service.handleResolve(request);

      assertThat(capturedContext.get().getValue("premium").asBoolean()).isTrue();
      assertThat(capturedContext.get().getValue("trial").asBoolean()).isFalse();
    }

    @Test
    void shouldExtractListValuesFromContext() {
      String requestBody =
          """
          {
            "flags": ["test-flag"],
            "evaluationContext": {
              "tags": ["a", "b", "c"]
            },
            "apply": false
          }
          """;

      AtomicReference<EvaluationContext> capturedContext = new AtomicReference<>();
      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenAnswer(
              invocation -> {
                capturedContext.set(invocation.getArgument(0));
                return ResolveFlagsResponse.getDefaultInstance();
              });

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      service.handleResolve(request);

      var tagsList = capturedContext.get().getValue("tags").asList();
      assertThat(tagsList).hasSize(3);
      assertThat(tagsList.get(0).asString()).isEqualTo("a");
      assertThat(tagsList.get(1).asString()).isEqualTo("b");
      assertThat(tagsList.get(2).asString()).isEqualTo("c");
    }

    @Test
    void shouldExtractComplexContextWithNestedStructsAndLists() {
      String requestBody =
          """
          {
            "flags": ["test-flag"],
            "evaluationContext": {
              "targeting_key": "user-xyz-789",
              "country": "SE",
              "age": 30,
              "premium": true,
              "score": 95.5,
              "tags": ["beta", "internal", "early-adopter"],
              "numbers": [1, 2, 3],
              "mixed_list": ["hello", 42, true],
              "user": {
                "id": "u123",
                "name": "Test User",
                "settings": {
                  "theme": "dark"
                }
              },
              "features": ["feature-a", "feature-b"]
            },
            "apply": true
          }
          """;

      AtomicReference<EvaluationContext> capturedContext = new AtomicReference<>();
      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenAnswer(
              invocation -> {
                capturedContext.set(invocation.getArgument(0));
                return ResolveFlagsResponse.getDefaultInstance();
              });

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      ConfidenceHttpResponse response = service.handleResolve(request);

      assertThat(response.getStatusCode()).isEqualTo(200);

      EvaluationContext ctx = capturedContext.get();

      // Verify targeting key
      assertThat(ctx.getTargetingKey()).isEqualTo("user-xyz-789");

      // Verify string value
      assertThat(ctx.getValue("country").asString()).isEqualTo("SE");

      // Verify number value
      assertThat(ctx.getValue("age").asDouble()).isEqualTo(30.0);
      assertThat(ctx.getValue("score").asDouble()).isEqualTo(95.5);

      // Verify boolean value
      assertThat(ctx.getValue("premium").asBoolean()).isTrue();

      // Verify string list
      var tagsList = ctx.getValue("tags").asList();
      assertThat(tagsList).hasSize(3);
      assertThat(tagsList.get(0).asString()).isEqualTo("beta");
      assertThat(tagsList.get(1).asString()).isEqualTo("internal");
      assertThat(tagsList.get(2).asString()).isEqualTo("early-adopter");

      // Verify number list
      var numbersList = ctx.getValue("numbers").asList();
      assertThat(numbersList).hasSize(3);
      assertThat(numbersList.get(0).asDouble()).isEqualTo(1.0);
      assertThat(numbersList.get(1).asDouble()).isEqualTo(2.0);
      assertThat(numbersList.get(2).asDouble()).isEqualTo(3.0);

      // Verify mixed list (numbers become doubles, booleans stay booleans)
      var mixedList = ctx.getValue("mixed_list").asList();
      assertThat(mixedList).hasSize(3);
      assertThat(mixedList.get(0).asString()).isEqualTo("hello");
      assertThat(mixedList.get(1).asDouble()).isEqualTo(42.0);
      assertThat(mixedList.get(2).asBoolean()).isTrue();

      // Verify nested struct is properly converted to Structure
      var userStruct = ctx.getValue("user").asStructure();
      assertThat(userStruct.getValue("id").asString()).isEqualTo("u123");
      assertThat(userStruct.getValue("name").asString()).isEqualTo("Test User");
      // Verify deeply nested struct
      var settingsStruct = userStruct.getValue("settings").asStructure();
      assertThat(settingsStruct.getValue("theme").asString()).isEqualTo("dark");

      // Verify features list
      var featuresList = ctx.getValue("features").asList();
      assertThat(featuresList).hasSize(2);
      assertThat(featuresList.get(0).asString()).isEqualTo("feature-a");
      assertThat(featuresList.get(1).asString()).isEqualTo("feature-b");

      // Verify the correct flags and apply value were passed
      verify(mockProvider).resolve(any(), eq(List.of("test-flag")), eq(true));
    }

    @Test
    void shouldReturn500WhenProviderThrowsException() {
      String requestBody =
          """
          {
            "flags": ["test-flag"],
            "evaluationContext": {},
            "apply": false
          }
          """;

      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenThrow(new RuntimeException("Provider error"));

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      ConfidenceHttpResponse response = service.handleResolve(request);

      assertThat(response.getStatusCode()).isEqualTo(500);
    }

    @Test
    void shouldHandleEmptyFlagsList() {
      String requestBody =
          """
          {
            "flags": [],
            "evaluationContext": {},
            "apply": false
          }
          """;

      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenReturn(ResolveFlagsResponse.getDefaultInstance());

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      ConfidenceHttpResponse response = service.handleResolve(request);

      assertThat(response.getStatusCode()).isEqualTo(200);
      verify(mockProvider).resolve(any(), eq(List.of()), eq(false));
    }

    @Test
    void shouldIgnoreUnknownFieldsInRequest() {
      String requestBody =
          """
          {
            "flags": ["test-flag"],
            "evaluationContext": {},
            "apply": false,
            "clientSecret": "should-be-ignored",
            "sdk": {"id": "ignored"},
            "unknownField": "also-ignored"
          }
          """;

      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenReturn(ResolveFlagsResponse.getDefaultInstance());

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      ConfidenceHttpResponse response = service.handleResolve(request);

      assertThat(response.getStatusCode()).isEqualTo(200);
    }
  }

  @Nested
  class HandleApply {

    @Test
    void shouldReturn405ForNonPostMethod() {
      ConfidenceHttpRequest request = createRequest("GET", "{}");

      ConfidenceHttpResponse response = service.handleApply(request);

      assertThat(response.getStatusCode()).isEqualTo(405);
    }

    @Test
    void shouldReturn400ForInvalidJson() {
      ConfidenceHttpRequest request = createRequest("POST", "invalid json");

      ConfidenceHttpResponse response = service.handleApply(request);

      assertThat(response.getStatusCode()).isEqualTo(400);
    }

    @Test
    void shouldApplyFlags() {
      String requestBody =
          """
          {
            "flags": [{"flag": "flags/my-flag", "applyTime": "2025-02-12T12:34:56Z"}],
            "resolveToken": "dGVzdC10b2tlbg=="
          }
          """;

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      ConfidenceHttpResponse response = service.handleApply(request);

      assertThat(response.getStatusCode()).isEqualTo(200);
      assertThat(readBody(response)).isEqualTo("{}");

      ArgumentCaptor<ApplyFlagsRequest> captor = ArgumentCaptor.forClass(ApplyFlagsRequest.class);
      verify(mockProvider).applyFlags(captor.capture());
      ApplyFlagsRequest captured = captor.getValue();
      assertThat(captured.getFlagsCount()).isEqualTo(1);
      assertThat(captured.getFlags(0).getFlag()).isEqualTo("flags/my-flag");
    }

    @Test
    void shouldApplyMultipleFlags() {
      String requestBody =
          """
          {
            "flags": [
              {"flag": "flags/flag1", "applyTime": "2025-02-12T12:34:56Z"},
              {"flag": "flags/flag2", "applyTime": "2025-02-12T12:34:57Z"}
            ],
            "resolveToken": "dGVzdC10b2tlbg=="
          }
          """;

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      ConfidenceHttpResponse response = service.handleApply(request);

      assertThat(response.getStatusCode()).isEqualTo(200);

      ArgumentCaptor<ApplyFlagsRequest> captor = ArgumentCaptor.forClass(ApplyFlagsRequest.class);
      verify(mockProvider).applyFlags(captor.capture());
      assertThat(captor.getValue().getFlagsCount()).isEqualTo(2);
    }

    @Test
    void shouldReturn500WhenProviderThrowsException() {
      String requestBody =
          """
          {
            "flags": [{"flag": "flags/test-flag"}],
            "resolveToken": "dGVzdA=="
          }
          """;

      doThrow(new RuntimeException("Apply failed")).when(mockProvider).applyFlags(any());

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      ConfidenceHttpResponse response = service.handleApply(request);

      assertThat(response.getStatusCode()).isEqualTo(500);
    }

    @Test
    void shouldHandleEmptyFlagsList() {
      String requestBody =
          """
          {
            "flags": [],
            "resolveToken": "dGVzdA=="
          }
          """;

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      ConfidenceHttpResponse response = service.handleApply(request);

      assertThat(response.getStatusCode()).isEqualTo(200);

      ArgumentCaptor<ApplyFlagsRequest> captor = ArgumentCaptor.forClass(ApplyFlagsRequest.class);
      verify(mockProvider).applyFlags(captor.capture());
      assertThat(captor.getValue().getFlagsCount()).isEqualTo(0);
    }
  }

  @Nested
  class ContextDecoration {

    @Test
    void shouldApplyContextDecorator() {
      ContextDecorator decorator =
          (ctx, req) -> {
            var userIds = req.getHeaders().get("X-User-Id");
            if (userIds != null && !userIds.isEmpty()) {
              ctx.add("decorated_user_id", userIds.get(0));
            }
            return CompletableFuture.completedFuture(null);
          };

      FlagResolverService serviceWithDecorator = new FlagResolverService(mockProvider, decorator);

      String requestBody =
          """
          {
            "flags": ["test-flag"],
            "evaluationContext": {},
            "apply": false
          }
          """;

      AtomicReference<EvaluationContext> capturedContext = new AtomicReference<>();
      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenAnswer(
              invocation -> {
                capturedContext.set(invocation.getArgument(0));
                return ResolveFlagsResponse.getDefaultInstance();
              });

      ConfidenceHttpRequest request =
          createRequestWithHeaders(
              "POST", requestBody, Map.of("X-User-Id", List.of("decorated-123")));
      serviceWithDecorator.handleResolve(request);

      assertThat(capturedContext.get().getValue("decorated_user_id").asString())
          .isEqualTo("decorated-123");
    }

    @Test
    void shouldCombineRequestContextWithDecorator() {
      ContextDecorator decorator =
          (ctx, req) -> {
            ctx.add("source", "backend_proxy");
            return CompletableFuture.completedFuture(null);
          };

      FlagResolverService serviceWithDecorator = new FlagResolverService(mockProvider, decorator);

      String requestBody =
          """
          {
            "flags": ["test-flag"],
            "evaluationContext": {
              "country": "SE"
            },
            "apply": false
          }
          """;

      AtomicReference<EvaluationContext> capturedContext = new AtomicReference<>();
      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenAnswer(
              invocation -> {
                capturedContext.set(invocation.getArgument(0));
                return ResolveFlagsResponse.getDefaultInstance();
              });

      ConfidenceHttpRequest request = createRequest("POST", requestBody);
      serviceWithDecorator.handleResolve(request);

      assertThat(capturedContext.get().getValue("country").asString()).isEqualTo("SE");
      assertThat(capturedContext.get().getValue("source").asString()).isEqualTo("backend_proxy");
    }
  }

  @Nested
  class MethodCaseInsensitivity {

    @Test
    void shouldAcceptLowercasePost() {
      String requestBody =
          """
          {"flags": [], "evaluationContext": {}, "apply": false}
          """;

      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenReturn(ResolveFlagsResponse.getDefaultInstance());

      ConfidenceHttpRequest request = createRequest("post", requestBody);
      ConfidenceHttpResponse response = service.handleResolve(request);

      assertThat(response.getStatusCode()).isEqualTo(200);
    }

    @Test
    void shouldAcceptMixedCasePost() {
      String requestBody =
          """
          {"flags": [], "evaluationContext": {}, "apply": false}
          """;

      when(mockProvider.resolve(any(), anyList(), anyBoolean()))
          .thenReturn(ResolveFlagsResponse.getDefaultInstance());

      ConfidenceHttpRequest request = createRequest("Post", requestBody);
      ConfidenceHttpResponse response = service.handleResolve(request);

      assertThat(response.getStatusCode()).isEqualTo(200);
    }
  }

  private ConfidenceHttpRequest createRequest(String method, String body) {
    return createRequestWithHeaders(method, body, Map.of());
  }

  private ConfidenceHttpRequest createRequestWithHeaders(
      String method, String body, Map<String, List<String>> headers) {
    return new ConfidenceHttpRequest() {
      @Override
      public String getMethod() {
        return method;
      }

      @Override
      public byte[] getBody() {
        return body.getBytes(StandardCharsets.UTF_8);
      }

      @Override
      public Map<String, List<String>> getHeaders() {
        return headers;
      }
    };
  }

  private String readBody(ConfidenceHttpResponse response) {
    byte[] body = response.getBody();
    if (body == null) {
      return null;
    }
    return new String(body, StandardCharsets.UTF_8);
  }
}
