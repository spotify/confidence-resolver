package com.spotify.confidence.sdk;

import com.google.protobuf.ByteString;
import com.google.protobuf.InvalidProtocolBufferException;
import com.google.protobuf.Struct;
import com.google.protobuf.util.JsonFormat;
import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsResponse;
import dev.openfeature.sdk.EvaluationContext;
import dev.openfeature.sdk.ImmutableContext;
import dev.openfeature.sdk.MutableContext;
import dev.openfeature.sdk.MutableStructure;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * HTTP service layer for the local resolve provider. Handles resolve and apply requests from client
 * SDKs (like confidence-sdk-js).
 *
 * <p>This service can proxy resolve/apply requests through the local provider, enabling low-latency
 * flag resolution without external network calls. Only {@code application/json} content type is
 * supported; requests with other content types will receive a 415 Unsupported Media Type response.
 *
 * <p><strong>Usage Example with Javalin:</strong>
 *
 * <pre>{@code
 * // Create provider
 * OpenFeatureLocalResolveProvider provider =
 *     new OpenFeatureLocalResolveProvider("client-secret");
 * OpenFeatureAPI.getInstance().setProviderAndWait(provider);
 *
 * // Create service with token sealing and optional context decoration
 * ResolveTokenSealer sealer = ResolveTokenSealer.create(System.getenv("CONFIDENCE_TOKEN_KEY"));
 * FlagResolverService flagResolver = new FlagResolverService(provider,
 *     ContextDecorator.sync((ctx, req) -> {
 *         // Set targeting key from auth middleware header
 *         List<String> userIds = req.headers().get("X-User-Id");
 *         if (userIds != null && !userIds.isEmpty()) {
 *             return ctx.merge(new ImmutableContext(userIds.get(0)));
 *         }
 *         return ctx;
 *     }),
 *     sealer);
 *
 * // Register endpoints
 * app.post("/v1/flags:resolve", ctx -> {
 *     ConfidenceHttpRequest request = new JavalinConfidenceHttpRequest(ctx);
 *     flagResolver.handleResolve(request).thenAccept(response -> {
 *         ctx.status(response.statusCode())
 *            .contentType("application/json")
 *            .result(response.body());
 *     });
 * });
 *
 * app.post("/v1/flags:apply", ctx -> {
 *     ConfidenceHttpRequest request = new JavalinConfidenceHttpRequest(ctx);
 *     flagResolver.handleApply(request).thenAccept(response -> {
 *         ctx.status(response.statusCode())
 *            .contentType("application/json")
 *            .result(response.body());
 *     });
 * });
 * }</pre>
 */
@Experimental
public class FlagResolverService<R extends ConfidenceHttpRequest> {
  private static final Logger log = LoggerFactory.getLogger(FlagResolverService.class);
  private static final JsonFormat.Printer JSON_PRINTER =
      JsonFormat.printer().omittingInsignificantWhitespace();
  private static final JsonFormat.Parser JSON_PARSER = JsonFormat.parser().ignoringUnknownFields();

  private final OpenFeatureLocalResolveProvider provider;
  private final ContextDecorator<R> contextDecorator;
  private final ResolveTokenSealer tokenSealer;

  /**
   * Creates a new FlagResolverService with no context decoration or token sealing.
   *
   * @param provider the local resolve provider to use for flag resolution
   */
  public FlagResolverService(OpenFeatureLocalResolveProvider provider) {
    this(provider, ContextDecorator.sync((ctx, req) -> ctx), null);
  }

  /**
   * Creates a new FlagResolverService with context decoration.
   *
   * @param provider the local resolve provider to use for flag resolution
   * @param contextDecorator decorator to add additional context from requests
   */
  public FlagResolverService(
      OpenFeatureLocalResolveProvider provider, ContextDecorator<R> contextDecorator) {
    this(provider, contextDecorator, null);
  }

  /**
   * Creates a new FlagResolverService with token sealing.
   *
   * <p>When a sealer is provided, the {@code resolve_token} in resolve responses is encrypted
   * (AES-256-GCM) so clients only see an opaque handle. The token is decrypted transparently when
   * it comes back via apply. This prevents clients from inspecting the raw resolve token which
   * carries the evaluation context and resolved variants.
   *
   * @param provider the local resolve provider to use for flag resolution
   * @param tokenSealer sealer for encrypting resolve tokens sent to clients
   */
  public FlagResolverService(
      OpenFeatureLocalResolveProvider provider, ResolveTokenSealer tokenSealer) {
    this(provider, ContextDecorator.sync((ctx, req) -> ctx), tokenSealer);
  }

  /**
   * Creates a new FlagResolverService with context decoration and token sealing.
   *
   * @param provider the local resolve provider to use for flag resolution
   * @param contextDecorator decorator to add additional context from requests
   * @param tokenSealer sealer for encrypting resolve tokens (may be {@code null} to disable)
   */
  public FlagResolverService(
      OpenFeatureLocalResolveProvider provider,
      ContextDecorator<R> contextDecorator,
      ResolveTokenSealer tokenSealer) {
    this.provider = provider;
    this.contextDecorator = contextDecorator;
    this.tokenSealer = tokenSealer;
  }

  /**
   * Handles POST /v1/flags:resolve requests from client SDKs.
   *
   * <p>Note: clientSecret and sdk fields from client are ignored - provider's credentials are used.
   *
   * @param request the incoming HTTP request
   * @return a CompletionStage that completes with the response to send back to the client
   */
  public CompletionStage<ConfidenceHttpResponse> handleResolve(R request) {
    // Validate HTTP method
    if (!"POST".equalsIgnoreCase(request.method())) {
      return CompletableFuture.completedFuture(ConfidenceHttpResponse.error(405));
    }

    // Validate content type
    if (!isJsonContentType(request)) {
      return CompletableFuture.completedFuture(ConfidenceHttpResponse.error(415));
    }

    return CompletableFuture.completedFuture(request)
        .thenCompose(
            req -> {
              // Parse request body
              final byte[] body = req.body();
              if (body == null || body.length == 0) {
                log.warn("Empty request body");
                return CompletableFuture.completedFuture(ConfidenceHttpResponse.error(400));
              }
              final String requestBody = new String(body, StandardCharsets.UTF_8);
              final ResolveFlagsRequest.Builder resolveRequestBuilder =
                  ResolveFlagsRequest.newBuilder();
              try {
                JSON_PARSER.merge(requestBody, resolveRequestBuilder);
              } catch (InvalidProtocolBufferException e) {
                log.warn("Invalid request format", e);
                return CompletableFuture.completedFuture(ConfidenceHttpResponse.error(400));
              }

              // Build evaluation context from request
              final EvaluationContext ctx =
                  buildEvaluationContext(resolveRequestBuilder.getEvaluationContext());

              // Apply context decorator and resolve flags
              return contextDecorator
                  .decorate(ctx, req)
                  .thenCompose(
                      decoratedCtx -> {
                        return provider.resolve(
                            decoratedCtx,
                            resolveRequestBuilder.getFlagsList(),
                            resolveRequestBuilder.getApply());
                      })
                  .thenApply(
                      response -> {
                        try {
                          final var sealed = sealResolveToken(response);
                          final String jsonResponse = JSON_PRINTER.print(sealed);
                          return ConfidenceHttpResponse.ok(jsonResponse);
                        } catch (InvalidProtocolBufferException e) {
                          log.warn("Invalid response format", e);
                          return ConfidenceHttpResponse.error(500);
                        }
                      });
            })
        .exceptionally(
            e -> {
              log.error("Error resolving flags", e);
              return ConfidenceHttpResponse.error(500);
            });
  }

  /**
   * Handles POST /v1/flags:apply requests from client SDKs.
   *
   * @param request the incoming HTTP request
   * @return a CompletionStage that completes with the response to send back to the client
   */
  public CompletionStage<ConfidenceHttpResponse> handleApply(R request) {
    // Validate HTTP method
    if (!"POST".equalsIgnoreCase(request.method())) {
      return CompletableFuture.completedFuture(ConfidenceHttpResponse.error(405));
    }

    // Validate content type
    if (!isJsonContentType(request)) {
      return CompletableFuture.completedFuture(ConfidenceHttpResponse.error(415));
    }

    try {
      // Parse request body
      final byte[] body = request.body();
      if (body == null || body.length == 0) {
        log.warn("Empty request body");
        return CompletableFuture.completedFuture(ConfidenceHttpResponse.error(400));
      }
      final String requestBody = new String(body, StandardCharsets.UTF_8);
      final ApplyFlagsRequest.Builder applyRequestBuilder = ApplyFlagsRequest.newBuilder();
      JSON_PARSER.merge(requestBody, applyRequestBuilder);

      // Open sealed token if a sealer is configured, then build the final request
      final ApplyFlagsRequest applyRequest = openResolveToken(applyRequestBuilder).build();

      // Apply each flag
      provider.applyFlags(applyRequest);

      // Return empty JSON response
      return CompletableFuture.completedFuture(ConfidenceHttpResponse.ok("{}"));

    } catch (InvalidProtocolBufferException e) {
      log.warn("Invalid request format", e);
      return CompletableFuture.completedFuture(ConfidenceHttpResponse.error(400));
    } catch (IllegalArgumentException e) {
      log.warn("Invalid resolve token", e);
      return CompletableFuture.completedFuture(ConfidenceHttpResponse.error(400));
    } catch (Exception e) {
      log.error("Error applying flags", e);
      return CompletableFuture.completedFuture(ConfidenceHttpResponse.error(500));
    }
  }

  private EvaluationContext buildEvaluationContext(Struct evaluationContext) {
    final MutableContext ctx = new MutableContext();

    evaluationContext
        .getFieldsMap()
        .forEach(
            (key, value) -> {
              switch (value.getKindCase()) {
                case STRING_VALUE -> {
                  if ("targeting_key".equals(key)) {
                    ctx.setTargetingKey(value.getStringValue());
                  } else {
                    ctx.add(key, value.getStringValue());
                  }
                }
                case NUMBER_VALUE -> ctx.add(key, value.getNumberValue());
                case BOOL_VALUE -> ctx.add(key, value.getBoolValue());
                case STRUCT_VALUE ->
                    ctx.add(key, protoStructToOpenFeatureStructure(value.getStructValue()));
                case LIST_VALUE -> {
                  // For lists, convert to List<Value>
                  final List<dev.openfeature.sdk.Value> valueList =
                      value.getListValue().getValuesList().stream()
                          .map(this::protoValueToOpenFeatureValue)
                          .toList();
                  ctx.add(key, valueList);
                }
                default ->
                    log.debug(
                        "Skipping unsupported value type for key {}: {}", key, value.getKindCase());
              }
            });

    return new ImmutableContext(ctx.getTargetingKey(), ctx.asMap());
  }

  private dev.openfeature.sdk.Value protoValueToOpenFeatureValue(com.google.protobuf.Value value) {
    return switch (value.getKindCase()) {
      case STRING_VALUE -> new dev.openfeature.sdk.Value(value.getStringValue());
      case NUMBER_VALUE -> new dev.openfeature.sdk.Value(value.getNumberValue());
      case BOOL_VALUE -> new dev.openfeature.sdk.Value(value.getBoolValue());
      case NULL_VALUE -> new dev.openfeature.sdk.Value();
      case STRUCT_VALUE ->
          new dev.openfeature.sdk.Value(protoStructToOpenFeatureStructure(value.getStructValue()));
      case LIST_VALUE -> {
        final List<dev.openfeature.sdk.Value> list =
            value.getListValue().getValuesList().stream()
                .map(this::protoValueToOpenFeatureValue)
                .toList();
        yield new dev.openfeature.sdk.Value(list);
      }
      default -> new dev.openfeature.sdk.Value(value.toString());
    };
  }

  private MutableStructure protoStructToOpenFeatureStructure(Struct struct) {
    final Map<String, dev.openfeature.sdk.Value> map = new HashMap<>();
    struct
        .getFieldsMap()
        .forEach((key, value) -> map.put(key, protoValueToOpenFeatureValue(value)));
    return new MutableStructure(map);
  }

  private ResolveFlagsResponse sealResolveToken(ResolveFlagsResponse response) {
    if (tokenSealer == null || response.getResolveToken().isEmpty()) {
      return response;
    }
    byte[] sealed = tokenSealer.seal(response.getResolveToken().toByteArray());
    return response.toBuilder().setResolveToken(ByteString.copyFrom(sealed)).build();
  }

  private ApplyFlagsRequest.Builder openResolveToken(ApplyFlagsRequest.Builder builder) {
    if (tokenSealer == null || builder.getResolveToken().isEmpty()) {
      return builder;
    }
    byte[] opened = tokenSealer.open(builder.getResolveToken().toByteArray());
    return builder.setResolveToken(ByteString.copyFrom(opened));
  }

  private static boolean isJsonContentType(ConfidenceHttpRequest request) {
    return request.headers().entrySet().stream()
        .filter(e -> e.getKey().equalsIgnoreCase("Content-Type"))
        .map(Map.Entry::getValue)
        .flatMap(List::stream)
        .findFirst()
        .map(ct -> ct.toLowerCase().startsWith("application/json"))
        .orElse(false);
  }
}
