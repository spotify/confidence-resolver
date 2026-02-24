# Confidence Android Provider ProGuard Rules

# Keep protobuf classes
-keep class com.google.protobuf.** { *; }
-keep class * extends com.google.protobuf.GeneratedMessageLite { *; }

# Keep gRPC classes
-keep class io.grpc.** { *; }
-keepnames class io.grpc.** { *; }

# Keep Chicory WASM runtime
-keep class com.dylibso.chicory.** { *; }

# Keep OpenFeature classes
-keep class dev.openfeature.** { *; }

# Keep our SDK classes
-keep class com.spotify.confidence.android.** { *; }
