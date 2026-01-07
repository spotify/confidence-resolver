package com.spotify.confidence.sdk;

import java.util.Arrays;
import java.util.List;
import java.util.regex.Pattern;
import org.slf4j.Logger;

class FlagPath {
  private static final Logger log = org.slf4j.LoggerFactory.getLogger(FlagPath.class);

  private final String flag;
  private final List<String> path;

  public FlagPath(String flag, List<String> path) {
    this.flag = flag;
    this.path = path;
  }

  public String getFlag() {
    return flag;
  }

  public List<String> getPath() {
    return path;
  }

  static FlagPath getPath(String str) throws Exceptions.IllegalValuePath {
    final String regex = Pattern.quote(".");
    final String[] parts = str.split(regex);

    if (parts.length == 0) {
      // this happens for malformed corner cases such as: str = "..."
      log.warn("Illegal path string '{}'", str);
      throw new Exceptions.IllegalValuePath(String.format("Illegal path string '%s'", str));
    } else if (parts.length == 1) {
      // str doesn't contain the delimiter
      return new FlagPath(str, List.of());
    } else {
      return new FlagPath(parts[0], Arrays.asList(parts).subList(1, parts.length));
    }
  }
}
