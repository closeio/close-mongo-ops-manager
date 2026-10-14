#!/usr/bin/env bash
#
# Build one fully static Linux release binary. Runs inside the builder image
# (docker/build/Dockerfile); invoked by build-release.sh:
#
#   build-linux-musl.sh <target> <bin-name> <out-dir>
set -euo pipefail

target=$1 bin_name=$2 out_dir=$3
bin=${CARGO_TARGET_DIR:-target}/$target/release/$bin_name

case $target in
    x86_64-unknown-linux-musl) machine='X86-64' ;;
    aarch64-unknown-linux-musl) machine='AArch64' ;;
    *)
        echo "error: unsupported target $target" >&2
        exit 2
        ;;
esac

cargo build --release --locked --target "$target"

file "$bin"
if ! readelf --file-header "$bin" | grep -q "Machine:.*$machine"; then
    echo "error: $bin is not a $machine binary" >&2
    exit 1
fi
# Fully static means no ELF interpreter and no shared library dependencies.
if readelf --program-headers --wide "$bin" | grep -q INTERP ||
    readelf --dynamic --wide "$bin" | grep -q '(NEEDED)'; then
    echo "error: $bin is dynamically linked" >&2
    exit 1
fi

install -m 0755 "$bin" "$out_dir/$bin_name"
# Bind mounts on Linux hosts would otherwise leave root-owned files behind.
if [[ -n ${HOST_UID:-} ]]; then
    chown -R "$HOST_UID:${HOST_GID:-$HOST_UID}" "$out_dir"
fi
