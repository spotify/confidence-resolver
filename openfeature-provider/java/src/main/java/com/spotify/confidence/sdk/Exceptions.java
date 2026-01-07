package com.spotify.confidence.sdk;

public class Exceptions {

  // Internal exceptions - package-private
  static class IllegalValuePath extends Exception {
    IllegalValuePath(String message) {
      super(message);
    }
  }

  static class ParseError extends RuntimeException {
    ParseError(String message) {
      super(message);
    }
  }
}
