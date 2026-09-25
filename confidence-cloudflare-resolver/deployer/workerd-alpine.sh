#!/bin/sh
set -eu
# Use glibc only for workerd; Node and the Rust build tools keep using musl.
deployer_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
set -- /opt/workerd-runtime/ld-linux-*.so.* --library-path /opt/workerd-runtime \
    "$deployer_dir/node_modules/workerd/bin/workerd" "$@"
exec "$@"
