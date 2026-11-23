package com.spotify.confidence.sdk.internal.host;

import com.spotify.confidence.flags.wasm.v1.ResolveProcessResponse.Error.Code;

/** A resolve the module rejected; the code follows gRPC status codes. */
public final class ResolveRejectException extends RuntimeException {

  private final Code code;

  ResolveRejectException(Code code, String message) {
    super("resolve rejected (" + code + "): " + message);
    this.code = code;
  }

  public Code code() {
    return code;
  }
}
