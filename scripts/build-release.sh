#!/usr/bin/env bash
#
# Build close-mongo-ops-manager release binaries and package them as
#
#   dist/close-mongo-ops-manager-<version>-<target>.tar.gz   (+ dist/SHA256SUMS)
#
#   aarch64-apple-darwin  native cargo build; needs a macOS host.
#   *-unknown-linux-musl  fully static binaries built in Docker (see
#                         docker/build/Dockerfile). Both arches are compiled on
#                         the Docker host's own arch, the foreign one by
#                         cross-compiling: far faster than rustc under QEMU.
#
# Unpacked binaries are kept in target/dist/<target>/. Run with --help for the
# options. Keep this compatible with the bash 3.2 that ships with macOS.

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
CRATE_DIR=$(cd "$SCRIPT_DIR/.." && pwd)
REPO_DIR=$(cd "$CRATE_DIR/.." && pwd)

MACOS_TARGETS=(aarch64-apple-darwin)
LINUX_TARGETS=(aarch64-unknown-linux-musl x86_64-unknown-linux-musl)

DOCKER=${DOCKER:-docker}
MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-11.0}
VERIFY_IMAGES=${VERIFY_IMAGES:-alpine:3 debian:stable-slim}

usage() {
    cat <<EOF
Usage: scripts/build-release.sh [options]

Build release binaries and package them into dist/ with a SHA256SUMS file.

Options:
  -t, --targets LIST  Targets to build, separated by commas or spaces
                      (default: \$TARGETS, else "all"):
                        ${MACOS_TARGETS[*]}
                        ${LINUX_TARGETS[*]}
                      Aliases: all, macos, linux, host.
      --native-only   Skip the targets built in Docker: build the macOS one
                      on a Mac, the host's own musl target on Linux (that one
                      needs musl-gcc, e.g. from musl-tools).
      --no-verify     Skip the smoke tests (running the binaries). Linux
                      binaries are always checked to be statically linked.
      --clean         Delete existing archives and SHA256SUMS first.
  -o, --out DIR       Output directory (default: dist/ in the crate).
  -h, --help          Show this help.

Environment:
  TARGETS                   Same as --targets.
  MACOSX_DEPLOYMENT_TARGET  Minimum macOS version (default: 11.0).
  RUST_IMAGE                Base image of the Linux builder
                            (default: rust:<rust-toolchain.toml channel>-alpine).
  VERIFY_IMAGES             Images the Linux binaries are smoke-tested in
                            (default: "alpine:3 debian:stable-slim").
  DOCKER                    Docker CLI (default: docker).
  CARGO_TARGET_DIR          Used by the native builds. Docker builds keep their
                            cargo registry and target dir in the volumes
                            <package>-cargo-registry and <package>-target-<arch>.
EOF
}

if [[ -t 2 ]]; then
    BOLD=$'\033[1m' RED=$'\033[31m' YELLOW=$'\033[33m' RESET=$'\033[0m'
else
    BOLD='' RED='' YELLOW='' RESET=''
fi
log() { printf '%s==> %s%s\n' "$BOLD" "$*" "$RESET" >&2; }
warn() { printf '%swarning:%s %s\n' "$YELLOW" "$RESET" "$*" >&2; }
die() {
    printf '%serror:%s %s\n' "$RED" "$RESET" "$*" >&2
    exit 1
}

# in_list <word> [<item>...]: whether <word> is one of the items.
in_list() {
    local word=$1 item
    shift
    for item in "$@"; do
        [[ $item != "$word" ]] || return 0
    done
    return 1
}

# toml_value <file> <section> <key>: a quoted string value from a plain TOML file.
toml_value() {
    awk -F'"' -v section="[$2]" -v key="$3" '
        /^[[:space:]]*\[/ {
            line = $0
            sub(/[[:space:]]*#.*$/, "", line)
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", line)
            in_section = (line == section)
            next
        }
        in_section && $1 ~ ("^[[:space:]]*" key "[[:space:]]*=[[:space:]]*$") { print $2; exit }
    ' "$1"
}

human_size() { awk -v bytes="$1" 'BEGIN { printf "%.1f MiB", bytes / 1048576 }'; }

file_size() { wc -c <"$1" | tr -d ' '; }

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$@"
    else
        shasum -a 256 "$@"
    fi
}

# --- Arguments ---------------------------------------------------------------

targets_spec=${TARGETS:-all}
native_only=0 verify=1 clean=0
dist_dir=$CRATE_DIR/dist

while (($#)); do
    case $1 in
        -t | --targets)
            (($# >= 2)) || die "$1 needs a value"
            targets_spec=$2
            shift 2
            ;;
        --targets=*) targets_spec=${1#*=} && shift ;;
        --native-only) native_only=1 && shift ;;
        --no-verify) verify=0 && shift ;;
        --clean) clean=1 && shift ;;
        -o | --out)
            (($# >= 2)) || die "$1 needs a value"
            dist_dir=$2
            shift 2
            ;;
        --out=*) dist_dir=${1#*=} && shift ;;
        -h | --help) usage && exit 0 ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
done

HOST_OS=$(uname -s)
case $(uname -m) in
    arm64 | aarch64) HOST_ARCH=aarch64 ;;
    x86_64 | amd64) HOST_ARCH=x86_64 ;;
    *) HOST_ARCH=$(uname -m) ;;
esac
case $HOST_OS in
    Darwin) HOST_TARGET=$HOST_ARCH-apple-darwin ;;
    Linux) HOST_TARGET=$HOST_ARCH-unknown-linux-musl ;;
    *) die "unsupported build host: $HOST_OS" ;;
esac

VERSION=$(toml_value "$CRATE_DIR/Cargo.toml" package version)
BIN_NAME=$(toml_value "$CRATE_DIR/Cargo.toml" package name)
[[ -n $VERSION && -n $BIN_NAME ]] || die "could not read the package name and version from Cargo.toml"
TOOLCHAIN=''
if [[ -f $CRATE_DIR/rust-toolchain.toml ]]; then
    TOOLCHAIN=$(toml_value "$CRATE_DIR/rust-toolchain.toml" toolchain channel)
fi

# --- Target selection --------------------------------------------------------

requested=()
request() {
    local target
    for target in "$@"; do
        in_list "$target" ${requested[@]+"${requested[@]}"} || requested+=("$target")
    done
}

read -r -a specs <<<"$(printf '%s' "$targets_spec" | tr ',\n' '  ')"
for spec in ${specs[@]+"${specs[@]}"}; do
    case $spec in
        all)
            [[ $HOST_OS != Darwin ]] || request "${MACOS_TARGETS[@]}"
            request "${LINUX_TARGETS[@]}"
            ;;
        macos | darwin | apple) request "${MACOS_TARGETS[@]}" ;;
        linux | musl) request "${LINUX_TARGETS[@]}" ;;
        host)
            in_list "$HOST_TARGET" "${MACOS_TARGETS[@]}" "${LINUX_TARGETS[@]}" ||
                die "there is no release target for this host ($HOST_TARGET)"
            request "$HOST_TARGET"
            ;;
        *)
            in_list "$spec" "${MACOS_TARGETS[@]}" "${LINUX_TARGETS[@]}" ||
                die "unknown target '$spec' (see --help)"
            request "$spec"
            ;;
    esac
done

# Use a fixed build order and apply --native-only.
selected=()
for target in "${MACOS_TARGETS[@]}" "${LINUX_TARGETS[@]}"; do
    in_list "$target" ${requested[@]+"${requested[@]}"} || continue
    if [[ $target == *-apple-darwin && $HOST_OS != Darwin ]]; then
        die "$target can only be built on macOS"
    fi
    if ((native_only)) && [[ $target == *-linux-musl && $target != "$HOST_TARGET" ]]; then
        warn "skipping $target: it is built in Docker (--native-only)"
        continue
    fi
    selected+=("$target")
done
((${#selected[@]})) || die "nothing to build"

# --- Building ----------------------------------------------------------------

STAGE_DIR=$CRATE_DIR/target/dist
mkdir -p "$dist_dir" "$STAGE_DIR"
DIST_DIR=$(cd "$dist_dir" && pwd)
WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/$BIN_NAME-release.XXXXXX")
trap 'rm -rf "$WORK_DIR"' EXIT

NATIVE_TARGET_DIR=''
BUILDER_IMAGE=''
DOCKER_ARCH=''

uses_docker() { [[ $1 == *-linux-musl ]] && ((!native_only)); }

docker_ready() { command -v "$DOCKER" >/dev/null 2>&1 && "$DOCKER" info >/dev/null 2>&1; }

# Build the builder image (cached by Docker after the first run), once per run.
prepare_builder() {
    [[ -z $BUILDER_IMAGE ]] || return 0
    docker_ready ||
        die "Linux targets are built in Docker, which isn't available; start it or use --native-only"
    if [[ -z ${RUST_IMAGE:-} ]]; then
        [[ $TOOLCHAIN =~ ^[0-9]+\.[0-9]+(\.[0-9]+)?$ ]] ||
            die "rust-toolchain.toml must pin a Rust version to pick the builder image; or set RUST_IMAGE"
        RUST_IMAGE=rust:$TOOLCHAIN-alpine
    fi
    BUILDER_IMAGE=$BIN_NAME-builder:${RUST_IMAGE##*:}
    log "Preparing the builder image $BUILDER_IMAGE (from $RUST_IMAGE)"
    "$DOCKER" build --quiet --build-arg "RUST_IMAGE=$RUST_IMAGE" --tag "$BUILDER_IMAGE" \
        "$CRATE_DIR/docker/build" >/dev/null
    local rustc_version
    rustc_version=$("$DOCKER" run --rm "$BUILDER_IMAGE" rustc --version | awk '{ print $2 }')
    if [[ -n $TOOLCHAIN && $rustc_version != "$TOOLCHAIN" && $rustc_version != "$TOOLCHAIN".* ]]; then
        die "$RUST_IMAGE has rustc $rustc_version but rust-toolchain.toml pins $TOOLCHAIN"
    fi
    DOCKER_ARCH=$("$DOCKER" info --format '{{.Architecture}}')
}

ensure_rust_target() {
    command -v rustup >/dev/null 2>&1 || return 0
    local installed
    installed=$(cd "$CRATE_DIR" && rustup target list --installed)
    if ! grep -qx "$1" <<<"$installed"; then
        log "Installing the Rust target $1"
        (cd "$CRATE_DIR" && rustup target add "$1")
    fi
}

build_native() {
    local target=$1
    ensure_rust_target "$target"
    if [[ -z $NATIVE_TARGET_DIR ]]; then
        NATIVE_TARGET_DIR=$(cd "$CRATE_DIR" && cargo metadata --format-version 1 --no-deps |
            sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
        [[ -n $NATIVE_TARGET_DIR ]] || die "could not determine cargo's target directory"
    fi
    log "Building $target (cargo)"
    (
        cd "$CRATE_DIR"
        if [[ $target == *-apple-darwin ]]; then
            export MACOSX_DEPLOYMENT_TARGET
        fi
        cargo build --release --locked --target "$target"
    )
    mkdir -p "$STAGE_DIR/$target"
    # Replace the staged binary instead of overwriting it in place: on macOS,
    # rewriting a file that a running process maps invalidates its code
    # signature, and both that process and new runs of the file get killed.
    cp "$NATIVE_TARGET_DIR/$target/release/$BIN_NAME" "$STAGE_DIR/$target/.$BIN_NAME.new"
    mv -f "$STAGE_DIR/$target/.$BIN_NAME.new" "$STAGE_DIR/$target/$BIN_NAME"
    if [[ $target == *-linux-musl ]]; then
        check_static "$target" "$STAGE_DIR/$target/$BIN_NAME"
    fi
}

build_docker() {
    local target=$1 out=$STAGE_DIR/$1 tty=()
    prepare_builder
    log "Building $target (Docker on $DOCKER_ARCH)"
    rm -rf "$out"
    mkdir -p "$out"
    [[ ! -t 1 ]] || tty=(--tty)
    "$DOCKER" run --rm --init ${tty[@]+"${tty[@]}"} \
        --volume "$CRATE_DIR:/src:ro" \
        --volume "$BIN_NAME-cargo-registry:/usr/local/cargo/registry" \
        --volume "$BIN_NAME-target-$DOCKER_ARCH:/build/target" \
        --volume "$out:/out" \
        --env CARGO_TARGET_DIR=/build/target \
        --env "HOST_UID=$(id -u)" --env "HOST_GID=$(id -g)" \
        --workdir /src \
        "$BUILDER_IMAGE" \
        bash scripts/build-linux-musl.sh "$target" "$BIN_NAME" /out
    [[ -x $out/$BIN_NAME ]] || die "$target: the build produced no binary"
}

# The same check build-linux-musl.sh does, for Linux binaries built natively.
check_static() {
    local target=$1 bin=$2 info
    if command -v readelf >/dev/null 2>&1; then
        if readelf --program-headers --wide "$bin" | grep -q INTERP ||
            readelf --dynamic --wide "$bin" | grep -q '(NEEDED)'; then
            die "$target: $bin is dynamically linked"
        fi
    elif command -v file >/dev/null 2>&1; then
        info=$(file -b "$bin")
        case $info in
            *"statically linked"* | *"static-pie linked"*) ;;
            *) die "$target: $bin is not statically linked: $info" ;;
        esac
    else
        warn "$target: neither readelf nor file is available; static linking not checked"
    fi
}

# --- Verification ------------------------------------------------------------

# Verification results of the current target, for the summary.
checks=''

verify_macos() {
    local target=$1 bin=$2 archs libs minos flag
    archs=$(lipo -archs "$bin")
    [[ $archs == arm64 ]] || die "$target: expected arm64 code in $bin, lipo reports '$archs'"
    # Only libraries and frameworks that ship with macOS may be linked.
    libs=$(otool -L "$bin" | awk '!/:$/ && $1 !~ /^\/(usr\/lib|System\/Library)\// { print $1 }')
    [[ -z $libs ]] || die "$target: links libraries that are not part of macOS: $libs"
    minos=$(otool -l "$bin" | awk '$1 == "minos" { print $2; exit }')
    checks="macOS >= $minos"
    if [[ $HOST_TARGET != "$target" ]]; then
        warn "$target: can't run arm64 code on this $HOST_ARCH host; smoke test skipped"
        checks="$checks, not run"
        return 0
    fi
    for flag in --version --help; do
        "$bin" "$flag" >/dev/null || die "$target: '$flag' failed"
    done
    checks="$checks, runs"
}

verify_linux() {
    local target=$1 bin=$2 platform image flag ran=''
    case $target in
        aarch64-*) platform=linux/arm64 ;;
        *) platform=linux/amd64 ;;
    esac
    if ! docker_ready; then
        if [[ $target == "$HOST_TARGET" ]]; then
            for flag in --version --help; do
                "$bin" "$flag" >/dev/null || die "$target: '$flag' failed"
            done
            checks='static, runs on host'
        else
            warn "$target: no Docker to run it in; smoke test skipped"
            checks='static, not run'
        fi
        return 0
    fi
    for image in $VERIFY_IMAGES; do
        if ! "$DOCKER" run --rm --platform "$platform" "$image" true >/dev/null 2>&1; then
            warn "$target: can't run $platform containers of $image here; smoke test skipped"
            continue
        fi
        for flag in --version --help; do
            "$DOCKER" run --rm --network none --platform "$platform" \
                --volume "$(dirname "$bin"):/w:ro" "$image" "/w/$BIN_NAME" "$flag" >/dev/null ||
                die "$target: '$flag' failed in $image ($platform)"
        done
        ran="$ran ${image%%:*}"
    done
    checks="static, runs in:${ran:- nothing}"
}

# --- Packaging ---------------------------------------------------------------

# first_file <path>...: the first of the paths that is a file, if any.
first_file() {
    local path
    for path in "$@"; do
        if [[ -f $path ]]; then
            printf '%s\n' "$path"
            return
        fi
    done
}

README=$(first_file "$CRATE_DIR/README.md" "$REPO_DIR/README.md")
LICENSE=$(first_file "$CRATE_DIR/LICENSE" "$REPO_DIR/LICENSE")
[[ -n $LICENSE ]] || warn "no LICENSE file found; the archives won't include one"

# tar_dir <dir>: tar <dir> (relative to $WORK_DIR) to stdout without the
# builder's user names or macOS metadata.
tar_dir() {
    case $(tar --version 2>/dev/null) in
        *"GNU tar"*)
            tar -C "$WORK_DIR" -cf - --owner=0 --group=0 --numeric-owner --sort=name "$1"
            ;;
        *bsdtar*)
            COPYFILE_DISABLE=1 tar -C "$WORK_DIR" -cf - --uid 0 --gid 0 --numeric-owner \
                --no-xattrs --no-mac-metadata "$1"
            ;;
        *) tar -C "$WORK_DIR" -cf - "$1" ;;
    esac
}

package() {
    local target=$1 name=$BIN_NAME-$VERSION-$1
    rm -rf "${WORK_DIR:?}/$name"
    mkdir "$WORK_DIR/$name"
    cp "$STAGE_DIR/$target/$BIN_NAME" "$WORK_DIR/$name/"
    [[ -z $README ]] || cp "$README" "$WORK_DIR/$name/README.md"
    [[ -z $LICENSE ]] || cp "$LICENSE" "$WORK_DIR/$name/LICENSE"
    chmod 0755 "$WORK_DIR/$name/$BIN_NAME"
    tar_dir "$name" | gzip -9 -n >"$DIST_DIR/$name.tar.gz"
}

# --- Main --------------------------------------------------------------------

if ((clean)); then
    rm -f "$DIST_DIR/$BIN_NAME"-*.tar.gz "$DIST_DIR/SHA256SUMS"
fi

log "Building $BIN_NAME $VERSION for: ${selected[*]}"

summary=$(printf '%-28s %7s %10s %10s  %s' target build binary archive checks)
finish_target() {
    local target=$1 seconds=$2 bin=$STAGE_DIR/$1/$BIN_NAME
    checks='not verified'
    if ((verify)); then
        case $target in
            *-apple-darwin) verify_macos "$target" "$bin" ;;
            *) verify_linux "$target" "$bin" ;;
        esac
    fi
    if command -v file >/dev/null 2>&1; then
        log "$target: $(file -b "$bin" | head -n 1)"
    fi
    package "$target"
    summary=$(printf '%s\n%-28s %7s %10s %10s  %s' "$summary" "$target" "${seconds}s" \
        "$(human_size "$(file_size "$bin")")" \
        "$(human_size "$(file_size "$DIST_DIR/$BIN_NAME-$VERSION-$target.tar.gz")")" "$checks")
}

for target in "${selected[@]}"; do
    start=$SECONDS
    if uses_docker "$target"; then
        build_docker "$target"
    else
        build_native "$target"
    fi
    finish_target "$target" $((SECONDS - start))
done

shopt -s nullglob
archives=("$DIST_DIR/$BIN_NAME-$VERSION-"*.tar.gz)
shopt -u nullglob
(cd "$DIST_DIR" && sha256 "${archives[@]##*/}" >SHA256SUMS)

log "Done"
printf '%s\n\n' "$summary"
printf 'Archives: %s (%d in SHA256SUMS)\n' "$DIST_DIR" "${#archives[@]}"
printf 'Binaries: %s/<target>/%s\n' "$STAGE_DIR" "$BIN_NAME"
