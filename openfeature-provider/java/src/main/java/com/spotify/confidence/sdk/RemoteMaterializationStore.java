package com.spotify.confidence.sdk;

import com.google.protobuf.ByteString;
import com.google.protobuf.CodedInputStream;
import com.google.protobuf.CodedOutputStream;
import com.google.protobuf.WireFormat;
import com.spotify.confidence.sdk.MaterializationStore.ReadOp;
import com.spotify.confidence.sdk.MaterializationStore.ReadResult;
import com.spotify.confidence.sdk.MaterializationStore.WriteOp;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * The hosted materialization store, as the JS provider uses it: protobuf over HTTP on the resolver
 * gateway. The five small messages are encoded by hand to avoid generating the internal protos.
 *
 * <p>Requests authenticate with {@code Authorization: ClientSecret <clientSecret>} and time out
 * after {@value #READ_TIMEOUT_SECONDS} seconds for reads and {@value #WRITE_TIMEOUT_SECONDS} for
 * writes, overridable with {@code CONFIDENCE_MATERIALIZATION_READ_TIMEOUT_SECONDS} and {@code
 * CONFIDENCE_MATERIALIZATION_WRITE_TIMEOUT_SECONDS}.
 */
public final class RemoteMaterializationStore implements MaterializationStore {

  static final String DEFAULT_BASE_URL = "https://resolver.confidence.dev";
  static final double READ_TIMEOUT_SECONDS = 2;
  static final double WRITE_TIMEOUT_SECONDS = 5;

  private static final Logger log = LoggerFactory.getLogger(RemoteMaterializationStore.class);

  // Message fields (confidence.flags.resolver.v1, internal_api.proto).
  private static final int READ_REQUEST_OPS = 3; // ReadOperationsRequest.ops
  private static final int READ_OP_VARIANT = 1; // ReadOp.variant_read_op
  private static final int READ_OP_INCLUSION = 2; // ReadOp.inclusion_read_op
  private static final int READ_RESULT_RESULTS = 1; // ReadOperationsResult.results
  private static final int RESULT_VARIANT = 1; // ReadResult.variant_result (VariantData)
  private static final int RESULT_INCLUSION = 2; // ReadResult.inclusion_result (InclusionData)
  private static final int WRITE_REQUEST_OPS = 1; // WriteOperationsRequest.store_variant_op
  private static final int F_UNIT = 1;
  private static final int F_MATERIAL = 2;
  private static final int F_RULE = 3;
  // Field 3 is the rule string for a variant result and the included flag for an inclusion
  // result; the wire type tells them apart.
  private static final int F_VARIANT = 4; // VariantData.variant

  private final String baseUrl;
  private final String clientSecret;
  private final HttpClient httpClient;
  private final ExecutorService executor =
      Executors.newCachedThreadPool(
          r -> {
            Thread t = new Thread(r, "confidence-remote-materialization");
            t.setDaemon(true);
            return t;
          });

  /** Uses the production resolver gateway. */
  public RemoteMaterializationStore(String clientSecret) {
    this(clientSecret, DEFAULT_BASE_URL);
  }

  /** Uses a custom gateway base URL (for tests). */
  public RemoteMaterializationStore(String clientSecret, String baseUrl) {
    this(clientSecret, baseUrl, HttpClientFactory.defaultFactory().createHttpClient());
  }

  RemoteMaterializationStore(String clientSecret, String baseUrl, HttpClient httpClient) {
    this.clientSecret = clientSecret;
    this.baseUrl = baseUrl == null || baseUrl.isEmpty() ? DEFAULT_BASE_URL : baseUrl;
    this.httpClient = httpClient;
  }

  @Override
  public CompletionStage<List<ReadResult>> read(List<? extends ReadOp> ops) {
    if (ops.isEmpty()) {
      return CompletableFuture.completedFuture(List.of());
    }
    return CompletableFuture.supplyAsync(() -> readSync(ops), executor);
  }

  @Override
  public CompletionStage<Void> write(Set<? extends WriteOp> ops) {
    if (ops.isEmpty()) {
      return CompletableFuture.completedFuture(null);
    }
    return CompletableFuture.runAsync(() -> writeSync(ops), executor);
  }

  /** Stops accepting work and waits briefly for in-flight requests; further calls fail. */
  public void shutdown() {
    executor.shutdown();
    try {
      if (!executor.awaitTermination(5, TimeUnit.SECONDS)) {
        executor.shutdownNow();
      }
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      executor.shutdownNow();
    }
  }

  private List<ReadResult> readSync(List<? extends ReadOp> ops) {
    try {
      byte[] response =
          post(
              "readMaterializedOperations",
              encodeRead(ops),
              timeoutSeconds(
                  "CONFIDENCE_MATERIALIZATION_READ_TIMEOUT_SECONDS", READ_TIMEOUT_SECONDS));
      return decodeResults(ByteString.copyFrom(response));
    } catch (IOException e) {
      throw new RuntimeException(
          "materialization readMaterializedOperations: " + e.getMessage(), e);
    }
  }

  private void writeSync(Set<? extends WriteOp> ops) {
    byte[] body;
    try {
      body = encodeWrite(ops);
    } catch (IOException e) {
      throw new RuntimeException(
          "materialization writeMaterializedOperations: " + e.getMessage(), e);
    }
    try {
      post(
          "writeMaterializedOperations",
          body,
          timeoutSeconds(
              "CONFIDENCE_MATERIALIZATION_WRITE_TIMEOUT_SECONDS", WRITE_TIMEOUT_SECONDS));
    } catch (IOException e) {
      throw new RuntimeException(
          "materialization writeMaterializedOperations: " + e.getMessage(), e);
    }
  }

  private byte[] post(String method, byte[] body, double timeoutSeconds) throws IOException {
    HttpRequest request =
        HttpRequest.newBuilder()
            .uri(URI.create(baseUrl + "/v1/materialization:" + method))
            .timeout(Duration.ofMillis((long) (timeoutSeconds * 1000)))
            .header("Content-Type", "application/x-protobuf")
            .header("Authorization", "ClientSecret " + clientSecret)
            .POST(HttpRequest.BodyPublishers.ofByteArray(body))
            .build();
    HttpResponse<byte[]> response;
    try {
      response = httpClient.send(request, HttpResponse.BodyHandlers.ofByteArray());
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new IOException("interrupted", e);
    }
    if (response.statusCode() != 200) {
      throw new IOException("HTTP " + response.statusCode());
    }
    return response.body();
  }

  // ---------------------------------------------------------------- wire encoding

  private static byte[] encodeRead(List<? extends ReadOp> ops) throws IOException {
    ByteArrayOutputStream buffer = new ByteArrayOutputStream();
    CodedOutputStream out = CodedOutputStream.newInstance(buffer);
    for (ReadOp op : ops) {
      int kind;
      byte[] inner;
      if (op instanceof ReadOp.Variant v) {
        inner = inner(F_UNIT, v.unit(), F_MATERIAL, v.materialization(), F_RULE, v.rule(), 0, null);
        kind = READ_OP_VARIANT;
      } else if (op instanceof ReadOp.Inclusion i) {
        inner = inner(F_UNIT, i.unit(), F_MATERIAL, i.materialization(), 0, null, 0, null);
        kind = READ_OP_INCLUSION;
      } else {
        throw new IOException("unsupported read op " + op.getClass().getName());
      }
      out.writeTag(READ_REQUEST_OPS, WireFormat.WIRETYPE_LENGTH_DELIMITED);
      // ReadOp{variant_read_op=1 | inclusion_read_op=2}: one length-delimited field.
      byte[] readOp = message(kind, inner);
      out.writeUInt32NoTag(readOp.length);
      out.writeRawBytes(readOp);
    }
    out.flush();
    return buffer.toByteArray();
  }

  private static byte[] encodeWrite(Set<? extends WriteOp> ops) throws IOException {
    ByteArrayOutputStream buffer = new ByteArrayOutputStream();
    CodedOutputStream out = CodedOutputStream.newInstance(buffer);
    for (WriteOp op : ops) {
      if (!(op instanceof WriteOp.Variant v)) {
        throw new IOException("unsupported write op " + op.getClass().getName());
      }
      byte[] data =
          inner(
              F_UNIT,
              v.unit(),
              F_MATERIAL,
              v.materialization(),
              F_RULE,
              v.rule(),
              F_VARIANT,
              v.variant());
      out.writeTag(WRITE_REQUEST_OPS, WireFormat.WIRETYPE_LENGTH_DELIMITED);
      out.writeUInt32NoTag(data.length);
      out.writeRawBytes(data);
    }
    out.flush();
    return buffer.toByteArray();
  }

  /** Writes the unit/materialization/(rule|included|variant) string fields of a small message. */
  private static byte[] inner(
      int unitField,
      String unit,
      int materializationField,
      String materialization,
      int ruleField,
      String rule,
      int variantField,
      String variant)
      throws IOException {
    ByteArrayOutputStream buffer = new ByteArrayOutputStream();
    CodedOutputStream out = CodedOutputStream.newInstance(buffer);
    appendString(out, unitField, unit);
    appendString(out, materializationField, materialization);
    appendString(out, ruleField, rule);
    appendString(out, variantField, variant);
    out.flush();
    return buffer.toByteArray();
  }

  private static void appendString(CodedOutputStream out, int field, String value)
      throws IOException {
    if (value == null || value.isEmpty()) {
      return;
    }
    out.writeTag(field, WireFormat.WIRETYPE_LENGTH_DELIMITED);
    out.writeStringNoTag(value);
  }

  private static byte[] message(int field, byte[] payload) throws IOException {
    ByteArrayOutputStream buffer = new ByteArrayOutputStream();
    CodedOutputStream out = CodedOutputStream.newInstance(buffer);
    out.writeTag(field, WireFormat.WIRETYPE_LENGTH_DELIMITED);
    out.writeUInt32NoTag(payload.length);
    out.writeRawBytes(payload);
    out.flush();
    return buffer.toByteArray();
  }

  // ---------------------------------------------------------------- wire decoding

  private static List<ReadResult> decodeResults(ByteString body) throws IOException {
    List<ReadResult> results = new ArrayList<>();
    CodedInputStream in = body.newCodedInput();
    while (!in.isAtEnd()) {
      int tag = in.readTag();
      if (WireFormat.getTagFieldNumber(tag) == READ_RESULT_RESULTS) {
        results.add(decodeResult(in.readBytes()));
      } else {
        in.skipField(tag);
      }
    }
    return results;
  }

  private static ReadResult decodeResult(ByteString message) throws IOException {
    CodedInputStream in = message.newCodedInput();
    String unit = "";
    String materialization = "";
    String rule = "";
    String variant = "";
    Boolean included = null;
    while (!in.isAtEnd()) {
      int tag = in.readTag();
      switch (WireFormat.getTagFieldNumber(tag)) {
        case RESULT_VARIANT, RESULT_INCLUSION -> {
          if (included != null || !variant.isEmpty()) {
            throw new IOException("malformed materialization response: multiple results");
          }
          ByteString data = in.readBytes();
          CodedInputStream fields = data.newCodedInput();
          while (!fields.isAtEnd()) {
            int fieldTag = fields.readTag();
            switch (WireFormat.getTagFieldNumber(fieldTag)) {
              case F_UNIT -> unit = fields.readStringRequireUtf8();
              case F_MATERIAL -> materialization = fields.readStringRequireUtf8();
              case F_RULE -> {
                if (WireFormat.getTagWireType(fieldTag) == WireFormat.WIRETYPE_VARINT) {
                  included = fields.readBool();
                } else {
                  rule = fields.readStringRequireUtf8();
                }
              }
              case F_VARIANT -> variant = fields.readStringRequireUtf8();
              default -> fields.skipField(fieldTag);
            }
          }
        }
        default -> in.skipField(tag);
      }
    }
    // An empty variant is "no assignment", which the store adapter drops — as in Go's
    // decoder, where an empty message decodes to a variant result with an empty variant.
    return included != null
        ? new ReadResult.Inclusion(materialization, unit, included)
        : new ReadResult.Variant(
            materialization,
            unit,
            rule,
            variant.isEmpty() ? Optional.empty() : Optional.of(variant));
  }

  private static double timeoutSeconds(String envVar, double fallback) {
    String raw = System.getenv(envVar);
    if (raw == null || raw.isBlank()) {
      return fallback;
    }
    try {
      double parsed = Double.parseDouble(raw.trim());
      if (parsed > 0) {
        return parsed;
      }
    } catch (NumberFormatException e) {
      // fall through to the warning
    }
    log.warn("Invalid {} value {}; using {}s", envVar, raw, fallback);
    return fallback;
  }
}
