#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Local CI and unsigned release builds. Does not publish or create signing keys.
set -euo pipefail
export COPYFILE_DISABLE=1
umask 022
repo=$(cd "$(dirname "$0")" && pwd -P)
cd "$repo"

fail() { printf 'cicd: %s\n' "$*" >&2; exit 1; }
usage() {
    cat <<'HELP'
Usage:
  ./cicd.sh check
  ./cicd.sh build [--targets host|all|TRIPLE[,TRIPLE...]]
                  [--out DIR] [--cache-dir DIR] [--linux-image IMAGE]

check runs the full tools/gate.sh. build runs the gate, freezes the committed
source with vendored dependencies, then builds unsigned release candidates.
The version comes from Cargo.toml. The checkout must be clean.

Targets: aarch64-apple-darwin, x86_64-apple-darwin,
         aarch64-unknown-linux-gnu, x86_64-unknown-linux-gnu.
Default: host. On macOS, all means all four; on Linux, both Linux targets.
macOS builds need the matching Rust target, Apple SDK and hdiutil.
macOS candidates include an unsigned DMG with an offline user-local installer. Linux builds use
Docker (default image rust:1.90.0-bookworm). Cross-architecture smoke checks
require Rosetta or Docker emulation. They do not qualify sandbox behavior.

Default output: dist/declass-VERSION-unsigned-COMMIT
Default cache:  ${CARGO_TARGET_DIR:-target}/cicd
Output must not exist. Use external cache/output directories for large builds.
No tag, GitHub release, signature or upload is created.
HELP
}

action=${1:---help}
[ $# -eq 0 ] || shift
case "$action" in
    -h|--help) usage; exit 0 ;;
    check) [ $# -eq 0 ] || fail 'check takes no arguments'; exec tools/gate.sh ;;
    build) ;;
    *) fail "unknown command: $action (use --help)" ;;
esac
targets=host
out=""
cache="${CARGO_TARGET_DIR:-$repo/target}/cicd"
linux_image=rust:1.90.0-bookworm
while [ $# -gt 0 ]; do
    case "$1" in
        --targets|--out|--cache-dir|--linux-image)
            [ $# -ge 2 ] && [ -n "$2" ] || fail "$1 requires a value"
            case "$1" in
                --targets) targets=$2 ;;
                --out) out=$2 ;;
                --cache-dir) cache=$2 ;;
                --linux-image) linux_image=$2 ;;
            esac
            shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) fail "unknown option: $1" ;;
    esac
done
for tool in git cargo rustc tar python3; do command -v "$tool" >/dev/null || fail "$tool is required"; done
[ -z "$(git status --porcelain)" ] || fail 'build requires a clean, committed checkout'
version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || fail 'invalid workspace version'
commit=$(git rev-parse HEAD)
epoch=$(git log -1 --format=%ct)
host=$(rustc -vV | sed -n 's/^host: //p')
case "$host" in
    aarch64-apple-darwin|x86_64-apple-darwin|aarch64-unknown-linux-gnu|x86_64-unknown-linux-gnu) ;;
    *) fail "unsupported build host: $host" ;;
esac
case "$targets" in
    host) targets=$host ;;
    all)
        targets=aarch64-unknown-linux-gnu,x86_64-unknown-linux-gnu
        case "$host" in *-apple-darwin) targets=aarch64-apple-darwin,x86_64-apple-darwin,$targets ;; esac ;;
esac
[[ "$targets" != ,* && "$targets" != *, && "$targets" != *,,* ]] || fail 'empty target in --targets'
IFS=, read -r -a matrix <<<"$targets"
seen=,
for target in "${matrix[@]}"; do
    case "$target" in
        aarch64-apple-darwin|x86_64-apple-darwin)
            [[ "$host" == *-apple-darwin ]] || fail 'macOS targets require a macOS host'
            command -v hdiutil >/dev/null || fail 'hdiutil is required for macOS DMGs'
            libdir=$(rustc --print target-libdir --target "$target")
            [ -d "$libdir" ] || fail "install the $target standard library for the active Rust toolchain" ;;
        aarch64-unknown-linux-gnu|x86_64-unknown-linux-gnu)
            command -v docker >/dev/null || fail 'Docker is required for Linux builds' ;;
        *) fail "unsupported target: $target" ;;
    esac
    [[ "$seen" != *",$target,"* ]] || fail "duplicate target: $target"
    seen=$seen$target,
done
out=${out:-$repo/dist/declass-$version-unsigned-${commit:0:12}}
[ ! -e "$out" ] && [ ! -L "$out" ] || fail "output already exists: $out"
# Resolve paths before creating anything; reject aliases and Docker mount syntax.
read_path() { python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$1"; }
out=$(read_path "$out")
cache=$(read_path "$cache")
case "$out$cache" in *$'\n'*|*','*) fail 'output/cache paths cannot contain newlines or commas' ;; esac
[ ! -e "$out" ] && [ ! -L "$out" ] || fail "output already exists: $out"
case "$out/" in "$cache/"*) fail 'output must not be inside the cache' ;; esac
case "$cache/" in "$out/"*) fail 'cache must not be inside the output' ;; esac
# Nonignored output in the checkout would invalidate the clean source snapshot.
for path in "$out" "$cache"; do
    case "$path" in "$repo"/*)
        git check-ignore -q "${path#"$repo"/}/probe" || fail 'in-repository output/cache must be ignored (use dist/ or target/)' ;;
    esac
done
mkdir -p "$(dirname "$out")"
mkdir "$out" || fail "cannot reserve fresh output directory: $out"
mkdir -p "$cache/tmp" "$out/logs" "$out/assets" "$out/releases"
export TMPDIR="$cache/tmp" SOURCE_DATE_EPOCH="$epoch"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$cache/host-target}"
CARGO_TARGET_DIR=$(read_path "$CARGO_TARGET_DIR")
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
# Avoid caller target overrides accidentally cross-compiling the host gate/SBOM.
unset CARGO_BUILD_TARGET
printf '== full host gate (%s); log: %s/logs/gate.log\n' "$host" "$out"
tools/gate.sh >"$out/logs/gate.log" 2>&1 || fail "gate failed; see $out/logs/gate.log"
[ "$(git rev-parse HEAD)" = "$commit" ] && [ -z "$(git status --porcelain)" ] || fail 'checkout changed during the gate'
printf '== corresponding source\n'
cargo fetch --locked >"$out/logs/fetch.log" 2>&1
source_archive="$out/declass-$version-source.tar.gz"
tools/source-release.sh "$source_archive" >"$out/logs/source.log" 2>&1
[ "$(git rev-parse HEAD)" = "$commit" ] && [ -z "$(git status --porcelain)" ] || fail 'checkout changed during source packaging'
scratch=$(mktemp -d "$cache/source.XXXXXXXX")
trap 'rm -rf "$scratch"' EXIT
tar -xzf "$source_archive" -C "$scratch"
source_dir="$scratch/declass-source"
for target in "${matrix[@]}"; do
    payload="$out/releases/$target"
    mkdir "$payload"
    printf '== build and smoke check %s; log: %s/logs/%s.log\n' "$target" "$out" "$target"
    case "$target" in
        *-apple-darwin)
            DECLASS_BUILD_ENVIRONMENT="macOS host $host; cross-architecture execution may use Rosetta" \
                "$source_dir/tools/build-candidate.sh" "$source_dir" "$target" "$version" "$commit" "$epoch" "$payload" \
                >"$out/logs/$target.log" 2>&1 || fail "build failed; see $out/logs/$target.log" ;;
        *-unknown-linux-gnu)
            case "$target" in aarch64-*) platform=linux/arm64 ;; *) platform=linux/amd64 ;; esac
            docker pull --platform "$platform" "$linux_image" >"$out/logs/$target-image.log" 2>&1
            image_id=$(docker image inspect "$linux_image" --format '{{.Id}}')
            mkdir -p "$cache/$target"
            docker run --rm --platform "$platform" --network none \
                --mount "type=bind,source=$source_dir,target=/src,readonly" \
                --mount "type=bind,source=$cache/$target,target=/cache" \
                --mount "type=bind,source=$payload,target=/out" \
                --env CARGO_HOME=/cache/cargo-home --env CARGO_TARGET_DIR=/cache/build \
                --env CARGO_BUILD_JOBS --env "DECLASS_BUILD_ENVIRONMENT=Docker $platform image $image_id; cross-architecture execution may use emulation" \
                --workdir /src "$image_id" bash tools/build-candidate.sh /src "$target" "$version" "$commit" "$epoch" /out \
                >"$out/logs/$target.log" 2>&1 || fail "build failed; see $out/logs/$target.log" ;;
    esac
    cp "$source_archive" "$payload/"
    cp "$source_dir/LICENSE" "$source_dir/NOTICE" "$source_dir/LICENSES.md" "$payload/"
    cp "$source_dir/crates/declass-boundary/rules/LICENSE.gitleaks" "$payload/"
    cp "$source_dir/crates/declass-boundary/rules/NOTICE" "$payload/NOTICE.gitleaks"
    cat >"$payload/SOURCE.txt" <<SOURCE
Corresponding Source for declass $version ($target)
Commit: $commit
Archive: declass-$version-source.tar.gz

Extract the archive and enter declass-source/. Locked Cargo dependencies are in
vendor/; license notices are in vendor/ and licenses/third-party/.
Install the Rust toolchain in BUILDINFO.txt,
the target standard library and platform C/C++ tools, then run offline:
  DECLASS_RELEASE_BUILD=1 cargo build --release --frozen -p declass-cli --bin declass --target $target
The binary is target/$target/release/declass. Linux runtime also needs bubblewrap.
Distribute this source archive and all license/notice files with the binary.
SOURCE
    python3 - "$payload" <<'PY'
import hashlib, pathlib, sys
p = pathlib.Path(sys.argv[1])
with (p / 'SHA256SUMS').open('w') as out:
    for item in sorted(p.iterdir()):
        if item.name == 'SHA256SUMS': continue
        with item.open('rb') as f: digest = hashlib.file_digest(f, 'sha256').hexdigest() if hasattr(hashlib, 'file_digest') else hashlib.sha256(f.read()).hexdigest()
        out.write(f'{digest}  {item.name}\n')
PY
    stem="declass-$version-$target-unsigned"
    tar -czf "$out/assets/$stem.tar.gz" -C "$payload" .
    if command -v shasum >/dev/null; then
        (cd "$out/assets" && shasum -a 256 "$stem.tar.gz" >"$stem.SHA256SUMS")
    else
        (cd "$out/assets" && sha256sum "$stem.tar.gz" >"$stem.SHA256SUMS")
    fi
    case "$target" in
        *-apple-darwin)
            "$source_dir/tools/package-dmg.sh" "$payload" --unsigned --out "$out/assets/$stem.dmg" \
                >"$out/logs/$target-dmg.log" 2>&1 || fail "DMG packaging failed; see $out/logs/$target-dmg.log" ;;
    esac
done
cat >"$out/README.txt" <<INFO
Declass $version — unsigned release candidates
Commit: $commit
Targets: $targets
Full host gate passed on $host. Each binary passed --version and --help.
Cross-architecture smoke checks may use Rosetta or Docker user-mode emulation;
they do not establish native sandbox correctness. Logs are under logs/.
No signature, notarization, release tag or publication was created.
macOS DMGs include an offline installer requiring unsigned confirmation.
Unsigned checksums detect changed bytes but do not authenticate a publisher.
Unsigned previews may be published with their unsigned status explicit.
Complete the required platform qualification before release. Signing is optional:
for publisher-authenticated artifacts, sign each releases/TARGET/SHA256SUMS with
the approved owner key, verify it, and use package-release.sh or package-dmg.sh.
See docs/INSTALLATION.md in the matching source archive for the procedure.
INFO
printf '\nUnsigned candidates built: %s\n' "$out"
