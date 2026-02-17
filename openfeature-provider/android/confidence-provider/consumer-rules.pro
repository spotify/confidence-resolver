# Consumer ProGuard Rules for Confidence Android Provider
# These rules are automatically included in consumer apps

# Keep protobuf generated classes
-keep class * extends com.google.protobuf.GeneratedMessageLite { *; }

# Keep gRPC service stubs
-keepclassmembers class * extends io.grpc.stub.AbstractStub { *; }
