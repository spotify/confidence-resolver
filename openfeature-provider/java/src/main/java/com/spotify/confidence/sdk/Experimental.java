package com.spotify.confidence.sdk;

import java.lang.annotation.Documented;
import java.lang.annotation.ElementType;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.lang.annotation.Target;

@Documented
@Retention(RetentionPolicy.RUNTIME)
@Target(ElementType.TYPE)
public @interface Experimental {
  String value() default
      "This class is experimental and not stable,"
          + " please avoid using it before talking to the confidence team.";
}
