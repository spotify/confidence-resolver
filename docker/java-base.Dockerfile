# Pre-baked Java base image with maven + protobuf-compiler + make.
# Built separately so the main Dockerfile skips the ~37s apt-get install.
FROM eclipse-temurin:17-jdk

RUN apt-get update && apt-get install -y \
    maven \
    protobuf-compiler \
    make \
  && rm -rf /var/lib/apt/lists/*
