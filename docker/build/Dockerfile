# syntax=docker/dockerfile:1
#
# Builder image for the fully static Linux (musl) release binaries; used by
# scripts/build-release.sh, which passes RUST_IMAGE derived from
# rust-toolchain.toml.
#
# Both Linux targets are compiled from the Docker host's own architecture, the
# foreign one by cross-compiling instead of running rustc under QEMU:
#   - C code (ring, zstd-sys) is compiled by clang against a per-arch musl
#     sysroot assembled from Alpine's own musl-dev packages, for both arches;
#   - the foreign arch is linked by rust-lld, the native one by the default
#     cc; either way rustc uses its self-contained musl CRT and libc, so the
#     result is a static-pie executable with no runtime dependencies.
# The native arch can't use rust-lld too: when target == host, cargo applies
# the target linker to proc-macros, which are dylibs linked against the
# system libc.
ARG RUST_IMAGE=rust:1.99.0-alpine
FROM ${RUST_IMAGE}

RUN apk add --no-cache bash clang file llvm

RUN rustup target add aarch64-unknown-linux-musl x86_64-unknown-linux-musl

RUN set -eu; \
    for arch in aarch64 x86_64; do \
        mkdir -p "/sysroot/$arch"; \
        apk add --quiet --no-cache --initdb --no-scripts \
            --root "/sysroot/$arch" --arch "$arch" \
            --keys-dir "/usr/share/apk/keys/$arch" \
            --repositories-file /etc/apk/repositories \
            musl-dev; \
    done

# Link the foreign arch with rust-lld (found next to rustc, no PATH needed).
RUN set -eu; \
    for arch in aarch64 x86_64; do \
        [ "$arch" = "$(uname -m)" ] && continue; \
        printf '[target.%s-unknown-linux-musl]\nlinker = "rust-lld"\n' "$arch" \
            >> "$CARGO_HOME/config.toml"; \
    done

# Use the image's toolchain even though the mounted sources have a
# rust-toolchain.toml (build-release.sh checks that both versions match).
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION} \
    CC_aarch64_unknown_linux_musl=clang \
    CFLAGS_aarch64_unknown_linux_musl=--sysroot=/sysroot/aarch64 \
    AR_aarch64_unknown_linux_musl=llvm-ar \
    CC_x86_64_unknown_linux_musl=clang \
    CFLAGS_x86_64_unknown_linux_musl=--sysroot=/sysroot/x86_64 \
    AR_x86_64_unknown_linux_musl=llvm-ar
