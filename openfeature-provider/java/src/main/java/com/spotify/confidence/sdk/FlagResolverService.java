package com.spotify.confidence.sdk;

import com.google.protobuf.InvalidProtocolBufferException;
import com.google.protobuf.Struct;
import com.google.protobuf.util.JsonFormat;
import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsResponse;
import dev.openfeature.sdk.MutableContext;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * HTTP service layer for the local resolve provider. Handles resolve and apply requests from client
 * SDKs (like confidence-sdk-js).
 *
 * <p>This service can proxy resolve/apply requests through the local provider, enabling low-latency
 * flag resolution without external network calls.
 *
 * <p><strong>Usage Example with Javalin:</strong>
 *
 * <pre>{@code
 * // Create provider
 * OpenFeatureLocalResolveProvider provider =
 *     new OpenFeatureLocalResolveProvider("client-secret");
 * OpenFeatureAPI.getInstance().setProviderAndWait(provider);
 *
 * // Create service with optional context decoration
 * FlagResolverService flagResolver = new FlagResolverService(provider, (ctx, req) -> {
 *     // Add user ID from auth header
 *     List<String> userIds = req.getHeaders().get("X-User-Id");
 *     if (userIds != null && !userIds.isEmpty()) {
 *         ctx.add("user_id", userIds.get(0));
 *     }
 * });
 *
 * // Register endpoints
 * app.post("/v1/flags:resolve", ctx -> {
 *     FlagResolverRequest request = new JavalinFlagResolverRequest(ctx);
 *     FlagResolverResponse response = flagResolver.handleResolve(request);
 *     ctx.status(response.getStatusCode())
 *        .contentType("application/json")
 *        .result(response.getBody());
 * });
 *
 * app.post("/v1/flags:apply", ctx -> {
 *     FlagResolverRequest request = new JavalinFlagResolverRequest(ctx);
 *     FlagResolverResponse response = flagResolver.handleApply(request);
 *     ctx.status(response.getStatusCode())
 *        .contentType("application/json")
 *        .result(response.getBody());
 * });
 * }</pre>
 */
@Experimental
public class FlagResolverService {
  private static final Logger log = LoggerFactory.getLogger(FlagResolverService.class);
  private static final JsonFormat.Printer JSON_PRINTER =
      JsonFormat.printer().omittingInsignificantWhitespace();
  private static final JsonFormat.Parser JSON_PARSER = JsonFormat.parser().ignoringUnknownFields();

  private final OpenFeatureLocalResolveProvider provider;
  private final ContextDecorator contextDecorator;

  /**
   * Creates a new FlagResolverService with no context decoration.
   *
   * @param provider the local resolve provider to use for flag resolution
   */
  public FlagResolverService(OpenFeatureLocalResolveProvider provider) {
    this(provider, (ctx, req) -> {});
  }

  /**
   * Creates a new FlagResolverService with context decoration.
   *
   * @param provider the local resolve provider to use for flag resolution
   * @param contextDecorator decorator to add additional context from requests
   */
  public FlagResolverService(
      OpenFeatureLocalResolveProvider provider, ContextDecorator contextDecorator) {
    this.provider = provider;
    this.contextDecorator = contextDecorator;
  }

  /**
   * Handles POST /v1/flags:resolve requests from client SDKs.
   *
   * <p>Note: clientSecret and sdk fields from client are ignored - provider's credentials are used.
   *
   * @param request the incoming HTTP request
   * @return the response to send back to the client
   */
  public FlagResolverResponse handleResolve(FlagResolverRequest request) {
    // Validate HTTP method
    if (!"POST".equalsIgnoreCase(request.getMethod())) {
      return FlagResolverResponse.error(405, "Method not allowed. Use POST.");
    }

    try {
      // Parse request body
      final String requestBody = readRequestBody(request.getBody());
      final ResolveFlagsRequest.Builder resolveRequestBuilder = ResolveFlagsRequest.newBuilder();
      JSON_PARSER.merge(requestBody, resolveRequestBuilder);

      // Build evaluation context from request
      final MutableContext ctx =
          buildEvaluationContext(resolveRequestBuilder.getEvaluationContext());

      // Apply context decorator
      contextDecorator.decorate(ctx, request);

      // Get flag names
      final List<String> flagNames = new ArrayList<>(resolveRequestBuilder.getFlagsList());

      // Resolve flags
      final ResolveFlagsResponse response =
          provider.resolve(ctx, flagNames, resolveRequestBuilder.getApply());

      // Convert response to JSON
      final String jsonResponse = JSON_PRINTER.print(response);
      return FlagResolverResponse.ok(jsonResponse);

    } catch (InvalidProtocolBufferException e) {
      log.warn("Invalid request format", e);
      return FlagResolverResponse.error(400, "Invalid request format: " + e.getMessage());
    } catch (IOException e) {
      log.error("Error reading request body", e);
      return FlagResolverResponse.error(500, "Error reading request body");
    } catch (Exception e) {
      log.error("Error resolving flags", e);
      return FlagResolverResponse.error(500, "Error resolving flags: " + e.getMessage());
    }
  }

  /**
   * Handles POST /v1/flags:apply requests from client SDKs.
   *
   * @param request the incoming HTTP request
   * @return the response to send back to the client
   */
  public FlagResolverResponse handleApply(FlagResolverRequest request) {
    // Validate HTTP method
    if (!"POST".equalsIgnoreCase(request.getMethod())) {
      return FlagResolverResponse.error(405, "Method not allowed. Use POST.");
    }

    try {
      // Parse request body
      final String requestBody = readRequestBody(request.getBody());
      final ApplyFlagsRequest.Builder applyRequestBuilder = ApplyFlagsRequest.newBuilder();
      JSON_PARSER.merge(requestBody, applyRequestBuilder);

      // Build the apply request - the resolve token is already in the protobuf
      final ApplyFlagsRequest applyRequest = applyRequestBuilder.build();

      // Apply each flag
      provider.applyFlags(applyRequest);

      // Return empty JSON response
      return FlagResolverResponse.ok("{}");

    } catch (InvalidProtocolBufferException e) {
      log.warn("Invalid request format", e);
      return FlagResolverResponse.error(400, "Invalid request format: " + e.getMessage());
    } catch (IOException e) {
      log.error("Error reading request body", e);
      return FlagResolverResponse.error(500, "Error reading request body");
    } catch (Exception e) {
      log.error("Error applying flags", e);
      return FlagResolverResponse.error(500, "Error applying flags: " + e.getMessage());
    }
  }

  private String readRequestBody(InputStream body) throws IOException {
    try (InputStreamReader reader = new InputStreamReader(body, StandardCharsets.UTF_8)) {
      StringBuilder sb = new StringBuilder();
      char[] buffer = new char[8192];
      int charsRead;
      while ((charsRead = reader.read(buffer)) != -1) {
        sb.append(buffer, 0, charsRead);
      }
      return sb.toString();
    }
  }

  private MutableContext buildEvaluationContext(Struct evaluationContext) {
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
                case STRUCT_VALUE -> {
                  // For nested structures, convert to string representation
                  try {
                    ctx.add(key, JSON_PRINTER.print(value.getStructValue()));
                  } catch (InvalidProtocolBufferException e) {
                    log.warn("Failed to convert struct value for key {}", key, e);
                  }
                }
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

    return ctx;
  }

  private dev.openfeature.sdk.Value protoValueToOpenFeatureValue(com.google.protobuf.Value value) {
    return switch (value.getKindCase()) {
      case STRING_VALUE -> new dev.openfeature.sdk.Value(value.getStringValue());
      case NUMBER_VALUE -> new dev.openfeature.sdk.Value(value.getNumberValue());
      case BOOL_VALUE -> new dev.openfeature.sdk.Value(value.getBoolValue());
      case NULL_VALUE -> new dev.openfeature.sdk.Value();
      default -> new dev.openfeature.sdk.Value(value.toString());
    };
  }
}
