#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Install signed releases when a trusted signer is configured; otherwise
# checksum-verified preview assets, with a notice.
# Usage: tools/install-release.sh [VERSION | --version VERSION] [--signers FILE]
#        [--url HTTPS_ASSET_DIRECTORY] [--allow-unsigned] [--no-modify-path]
#        (default version: latest stable release)
# DECLASS_INSTALL_DIR defaults to ~/.local/bin; DECLASS_DATA_DIR to ~/.local/share/declass.
# Defaults to GitHub Releases (maximpri/duet, tag vVERSION). The corresponding
# source, licenses and signed release records are retained locally.
set -euo pipefail
umask 077

fail() { printf 'declass install-release: %s\n' "$*" >&2; exit 1; }
usage() {
    cat <<'HELP'
Usage: install.sh [VERSION | --version VERSION] [--signers FILE]
                  [--url HTTPS_ASSET_DIRECTORY] [--allow-unsigned]
                  [--no-modify-path]
Defaults to the latest stable maximpri/duet GitHub release for this OS/CPU.
Trust file: $DECLASS_CONFIG_HOME/allowed_signers, or ~/.config/declass/allowed_signers.
Obtain that file through an independently trusted channel before installing.
With a trust file (or --signers), a valid signature is required: no fallback.
Without one, checksum-only preview assets are installed; checksums do not
authenticate the publisher. --allow-unsigned selects them even with a trust file.
Installs in ~/.local/bin and retains matching source and notices under
~/.local/share/declass/releases. DECLASS_INSTALL_DIR / DECLASS_DATA_DIR override those paths.
When ~/.local/bin is not on PATH, one marked line adding it is appended to your
shell's startup file; --no-modify-path (or DECLASS_NO_MODIFY_PATH=1) leaves it alone.
No sudo or downloaded signing key.
HELP
}
version=""
url=""
allow_unsigned=false
signers_given=false
modify_path=true
[ "${DECLASS_NO_MODIFY_PATH:-}" != 1 ] || modify_path=false
signers="${DECLASS_CONFIG_HOME:-$HOME/.config/declass}/allowed_signers"
while [ $# -gt 0 ]; do
    case "$1" in
    -h | --help) usage; exit 0 ;;
    --allow-unsigned) allow_unsigned=true; shift ;;
    --no-modify-path) modify_path=false; shift ;;
    --url | --signers | --version)
        [ $# -ge 2 ] && [ -n "$2" ] || fail "$1 requires a value"
        case "$1" in
            --url) url="$2" ;;
            --signers) signers="$2"; signers_given=true ;;
            --version) [ -z "$version" ] || fail "specify one version"; version="$2" ;;
        esac
        shift 2
        ;;
    -*) fail "unknown option: $1" ;;
    *) [ -z "$version" ] || fail "specify one version"; version="$1"; shift ;;
    esac
done
[ -n "$version" ] || version=latest
if [ "$version" != latest ]; then
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || fail "specify latest or a version as x.y.z[-pre]"
fi
# No trust file configured: nothing could authenticate a signature, so install
# the preview assets. A configured trust file always requires a signature.
if ! $allow_unsigned && ! $signers_given && [ ! -e "$signers" ]; then
    allow_unsigned=true
fi
if ! $allow_unsigned; then
    [ -f "$signers" ] && [ -r "$signers" ] && [ -s "$signers" ] || fail "no nonempty readable allowed-signers file at $signers; obtain the release signer through an independent trusted channel, or pass --signers FILE"
fi
for tool in curl uname mktemp cp chmod mv mkdir rm grep tar bash dirname; do
    command -v "$tool" >/dev/null 2>&1 || fail "required command not found: $tool"
done
if ! $allow_unsigned; then
    command -v ssh-keygen >/dev/null 2>&1 || fail "ssh-keygen is required for signed releases"
fi
command -v shasum >/dev/null 2>&1 || command -v sha256sum >/dev/null 2>&1 || fail "shasum or sha256sum is required"
verifier="$(cd "$(dirname "$0")" && pwd -P)/verify-release.sh"
[ -x "$verifier" ] || fail "verify-release.sh must be present beside this trusted installer"
checker="$(dirname "$verifier")/verify-checksums.sh"
[ -x "$checker" ] || fail "verify-checksums.sh must be present beside this trusted installer"
os=$(uname -s)
arch=$(uname -m)
# A translated shell on Apple Silicon should install the native ARM64 binary.
if [ "$os:$arch" = Darwin:x86_64 ] && command -v sysctl >/dev/null 2>&1; then
    translated=$(sysctl -in sysctl.proc_translated 2>/dev/null || true)
    [ "$translated" != 1 ] || arch=arm64
fi
case "$os:$arch" in
Darwin:arm64 | Darwin:aarch64) target=aarch64-apple-darwin ;;
Darwin:x86_64) target=x86_64-apple-darwin ;;
Linux:aarch64 | Linux:arm64) target=aarch64-unknown-linux-gnu ;;
Linux:x86_64 | Linux:amd64) target=x86_64-unknown-linux-gnu ;;
*) fail "supported platforms are macOS and GNU/Linux on ARM64 or x86-64" ;;
esac
install_dir="${DECLASS_INSTALL_DIR:-$HOME/.local/bin}"
data_dir="${DECLASS_DATA_DIR:-$HOME/.local/share/declass}"
for destination in "$install_dir" "$data_dir"; do
    case "$destination" in /*) ;; *) fail "DECLASS_INSTALL_DIR and DECLASS_DATA_DIR must be absolute paths" ;; esac
done
[ ! -d "$install_dir/declass" ] || fail "destination is a directory: $install_dir/declass"

if [ "$version" = latest ]; then
    # Resolve once; every asset then uses the same pinned version directory.
    # GitHub's latest endpoint excludes drafts and prereleases. No JSON parser,
    # API token or mutable latest/download URL is needed.
    resolved=$(curl --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --silent --show-error \
        --location --max-redirs 5 --connect-timeout 20 --max-time 60 --head \
        --output /dev/null --write-out '%{url_effective}' \
        https://github.com/maximpri/duet/releases/latest) ||
        fail "cannot resolve the latest stable GitHub release; none may be published, or GitHub is unavailable"
    case "$resolved" in
        https://github.com/maximpri/duet/releases/tag/v*) version=${resolved#https://github.com/maximpri/duet/releases/tag/v} ;;
        *) fail "no supported stable GitHub release found; expected a vX.Y.Z release tag" ;;
    esac
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "latest release is not a supported stable vX.Y.Z tag"
fi
[ -n "$url" ] || url="https://github.com/maximpri/duet/releases/download/v$version"
[[ "$url" =~ ^https://[^/?#]+(/[^?#]*)?$ ]] || fail "--url must be an HTTPS release-directory URL without query or fragment"
url=${url%/}
if $allow_unsigned; then
    printf 'Installing UNSIGNED Declass %s for %s: checksums verify integrity, not publisher identity.\n' "$version" "$target"
else
    printf 'Installing signed Declass %s for %s\n' "$version" "$target"
fi

scratch=$(mktemp -d "${TMPDIR:-/tmp}/declass-install-release.XXXXXXXX")
staged_binary=""
retained=""
cleanup() {
    rm -rf "$scratch"
    [ -z "$staged_binary" ] || rm -f "$staged_binary"
    [ -z "$retained" ] || rm -rf "$retained"
}
trap cleanup EXIT
binary="declass-$version-$target"
stem="$binary"
$allow_unsigned && stem="$stem-unsigned"
archive="$stem.tar.gz"
manifest="$stem.SHA256SUMS"
download() {
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --silent --show-error \
        --location --max-redirs 5 --connect-timeout 20 --max-time 600 \
        --output "$2" "$url/$1" || fail "download failed: $1"
}
verify_payload() {
    if $allow_unsigned; then
        # Never silently downgrade a signed asset or ignore a present bad signature.
        [ ! -e "$1/SHA256SUMS.sig" ] && [ ! -L "$1/SHA256SUMS.sig" ] || fail 'unsigned assets must not contain a signature'
        "$checker" "$1"
    else
        "$verifier" "$1" --signers "$signers"
    fi
}
# A signing key is supplied independently, never downloaded with the payload.
download "$manifest" "$scratch/SHA256SUMS"
$allow_unsigned || download "$manifest.sig" "$scratch/SHA256SUMS.sig"
download "$archive" "$scratch/$archive"
verify_payload "$scratch"
# Inspect the authenticated/checksummed archive before extracting anything. Both
# package-release.sh (name) and cicd.sh (./name) use flat regular payload files.
# Reject extras, duplicates, links and nested paths even in an unsigned preview.
names=("$binary" "declass-$version.cdx.json" BUILDINFO.txt LICENSE NOTICE LICENSES.md
    LICENSE.gitleaks NOTICE.gitleaks "declass-$version-source.tar.gz" SOURCE.txt SHA256SUMS)
$allow_unsigned || names+=(SHA256SUMS.sig)
listing="$scratch/entries"
details="$scratch/types"
tar -tzf "$scratch/$archive" >"$listing" || fail 'cannot list release archive'
tar -tvzf "$scratch/$archive" >"$details" || fail 'cannot inspect release archive'
seen=""
exec 3<"$details"
while IFS= read -r name; do
    IFS= read -r detail <&3 || fail 'inconsistent archive listing'
    flat=${name#./}
    if [ "$name" = . ] || [ "$name" = ./ ]; then
        case "$detail" in d*) continue ;; *) fail 'invalid archive root entry' ;; esac
    fi
    case "$detail" in -*) ;; *) fail "archive entry is not a regular file: $name" ;; esac
    allowed=false
    for expected in "${names[@]}"; do [ "$flat" != "$expected" ] || allowed=true; done
    $allowed || fail "unexpected release archive entry: $name"
    grep -qxF -- "$flat" <<<"$seen" && fail "duplicate release archive entry: $flat"
    seen="${seen}${seen:+$'\n'}$flat"
done <"$listing"
exec 3<&-
payload="$scratch/payload"
mkdir "$payload"
for name in "${names[@]}"; do
    grep -qxF -- "$name" <<<"$seen" || fail "release archive lacks expected file: $name"
    entry="$name"
    grep -qxF -- "$entry" "$listing" || entry="./$name"
    # stdout extraction never restores archive ownership, modes or paths.
    tar -xOf "$scratch/$archive" "$entry" >"$payload/$name" || fail "cannot extract expected file: $name"
done
verify_payload "$payload"
grep -qxF -- "declass $version" "$payload/BUILDINFO.txt" || fail "signed build metadata does not match version $version"
grep -qxF -- "target $target" "$payload/BUILDINFO.txt" || fail "signed build metadata does not match target $target"

mkdir -p "$install_dir"
staged_binary=$(mktemp "$install_dir/.declass.XXXXXXXX")
cp "$payload/$binary" "$staged_binary"
chmod 755 "$staged_binary"
# Required authentication (unless explicitly unsigned) and all hashes passed
# before any downloaded code runs.
reported_version=$("$staged_binary" --version) || fail "verified binary cannot run on this host; existing installation retained"
[ "$reported_version" = "declass $version" ] || fail "verified binary reports an unexpected version: $reported_version"
mkdir -p "$data_dir/releases"
retained=$(mktemp -d "$data_dir/releases/$version-$target.XXXXXXXX")
cp "$payload"/* "$retained/"
# Same-directory rename is atomic: download/verification/copy failures above
# never replace the existing executable. Existing release records are retained.
mv -f "$staged_binary" "$install_dir/declass"
staged_binary=""
installed_records="$retained"
retained=""
printf 'Installed %s at %s/declass\n' "$reported_version" "$install_dir"
if $allow_unsigned; then
    printf 'Checksum-verified source and records (unsigned): %s\n' "$installed_records"
else
    printf 'Verified source, licenses and release records: %s\n' "$installed_records"
fi
case ":$PATH:" in
    *":$install_dir:"*) printf '\nRun it in your project directory:  declass\n' ;;
    *)
        if ! $modify_path; then
            printf '\nAdd %s to PATH to run declass (no shell profile was changed).\n' "$install_dir"
        else
            case "${SHELL##*/}" in
                zsh) profile="${ZDOTDIR:-$HOME}/.zshrc" ;;
                bash) if [ "$os" = Darwin ]; then profile="$HOME/.bash_profile"; else profile="$HOME/.bashrc"; fi ;;
                fish) profile="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/declass.fish" ;;
                *) profile="$HOME/.profile" ;;
            esac
            if [ "${SHELL##*/}" = fish ]; then
                line="fish_add_path -g '$install_dir' # added by the declass installer"
            else
                line="export PATH=\"$install_dir:\$PATH\" # added by the declass installer"
            fi
            if [ -f "$profile" ] && grep -qF -- "$line" "$profile"; then
                printf '\n%s already adds %s to PATH.\n' "$profile" "$install_dir"
            else
                mkdir -p "$(dirname "$profile")"
                printf '\n%s\n' "$line" >>"$profile"
                printf '\nAdded %s to PATH in %s (remove the line marked "declass installer" to undo).\n' "$install_dir" "$profile"
            fi
            printf 'Open a new terminal, then run it in your project directory:  declass\n'
        fi
        ;;
esac
