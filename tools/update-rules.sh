#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Updates the imported detection rules: the gitleaks default rule set, vendored
# as data in crates/declass-boundary/rules/ (MIT; see rules/NOTICE).
#
#   tools/update-rules.sh <version>            fetch the rule file of a release tag,
#                                              verify it, show the rule diff (changes nothing)
#   tools/update-rules.sh <version> --apply    the same, then vendor it and rewrite rules/NOTICE
#   tools/update-rules.sh --check              verify the vendored file against rules/NOTICE (offline)
#
# Options: --sha256 <hex> pins the expected hash of the fetched file.
#
# Verification: the file is fetched from raw.githubusercontent.com at the tag and
# must equal the git blob the tag's tree holds, asked separately from the GitHub
# API; a pinned --sha256 must match; re-fetching the vendored version must give
# the vendored hash. After --apply, run the detection corpus, which lists rules
# that do not compile and fails if they, or false positives, grow:
#   cargo test -p declass-boundary --test corpus -- --nocapture
set -euo pipefail
cd "$(dirname "$0")/.."

repo=gitleaks/gitleaks
dir=crates/declass-boundary/rules
notice="$dir/NOTICE"

usage() {
    sed -n '3,19p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
}

field() { sed -n "s/^$1: *//p" "$notice" | head -1; }

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

version="" apply=false check=false pin=""
while [ $# -gt 0 ]; do
    case "$1" in
    --apply) apply=true ;;
    --check) check=true ;;
    --sha256)
        pin="${2:-}"
        shift
        ;;
    -h | --help) usage ;;
    v[0-9]*) version="$1" ;;
    *) usage ;;
    esac
    shift
done

if $check; then
    want=$(field sha256)
    have=$(sha256 "$dir/gitleaks.toml")
    if [ "$want" != "$have" ]; then
        echo "rules/gitleaks.toml does not match rules/NOTICE: $have != $want" >&2
        exit 1
    fi
    echo "rules/gitleaks.toml is $(field version), sha256 $have (as pinned in rules/NOTICE)"
    exit 0
fi
[ -n "$version" ] || usage
if ! printf '%s' "$version" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$'; then
    echo "a release tag like v8.30.1 is required" >&2
    exit 2
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "fetching $repo $version ..."
curl -fsSL "https://raw.githubusercontent.com/$repo/$version/config/gitleaks.toml" -o "$tmp/gitleaks.toml"
curl -fsSL "https://raw.githubusercontent.com/$repo/$version/LICENSE" -o "$tmp/LICENSE"
api_blob=$(curl -fsSL "https://api.github.com/repos/$repo/contents/config/gitleaks.toml?ref=$version" |
    sed -n 's/.*"sha": *"\([0-9a-f]\{40\}\)".*/\1/p' | head -1)
commit=$(curl -fsSL "https://api.github.com/repos/$repo/git/ref/tags/$version" |
    sed -n 's/.*"sha": *"\([0-9a-f]\{40\}\)".*/\1/p' | head -1)
blob=$(git hash-object "$tmp/gitleaks.toml")
if [ -z "$api_blob" ] || [ "$blob" != "$api_blob" ]; then
    echo "verification failed: the fetched file (git blob $blob) is not the tag's ($api_blob)" >&2
    exit 1
fi
hash=$(sha256 "$tmp/gitleaks.toml")
license_hash=$(sha256 "$tmp/LICENSE")
if [ -n "$pin" ] && [ "$hash" != "$pin" ]; then
    echo "verification failed: sha256 $hash, expected $pin" >&2
    exit 1
fi
if [ "$version" = "$(field version)" ] && [ "$hash" != "$(field sha256)" ]; then
    echo "verification failed: $version changed upstream since it was vendored ($hash != $(field sha256))" >&2
    exit 1
fi
if ! grep -q '^MIT License' "$tmp/LICENSE"; then
    echo "the license of $version is no longer MIT: review it before importing" >&2
    exit 1
fi
echo "verified: git blob $blob matches the tag's tree; sha256 $hash"

# One file per rule (named by id) plus the part before the first rule.
split_rules() {
    mkdir -p "$2"
    awk -v out="$2" '
        /^\[\[rules\]\]/ { if (f != "") close(f); f = ""; head = $0; next }
        head != "" && /^id = / {
            id = $3; gsub(/"/, "", id); f = out "/" id
            print head > f; print > f; head = ""; next
        }
        f != "" { print > f; next }
        { print > (out "/_global-allowlist") }
    ' "$1"
}
split_rules "$dir/gitleaks.toml" "$tmp/old"
split_rules "$tmp/gitleaks.toml" "$tmp/new"

added=$(comm -13 <(ls "$tmp/old") <(ls "$tmp/new"))
removed=$(comm -23 <(ls "$tmp/old") <(ls "$tmp/new"))
changed=""
for id in $(comm -12 <(ls "$tmp/old") <(ls "$tmp/new")); do
    cmp -s "$tmp/old/$id" "$tmp/new/$id" || changed="$changed $id"
done
count() { if [ -z "$1" ]; then echo 0; else echo "$1" | wc -w | tr -d ' '; fi; }
printf '\nrule diff %s -> %s: %s added, %s removed, %s changed (%s rules in %s)\n' \
    "$(field version)" "$version" "$(count "$added")" "$(count "$removed")" "$(count "$changed")" \
    "$(grep -c '^\[\[rules\]\]' "$tmp/gitleaks.toml")" "$version"
for id in $added; do echo "  + $id"; done
for id in $removed; do echo "  - $id"; done
for id in $changed; do
    echo "  ~ $id"
    diff -u "$tmp/old/$id" "$tmp/new/$id" | tail -n +3 | sed 's/^/      /'
done

if ! $apply; then
    printf '\nnothing changed; re-run with --apply to vendor %s\n' "$version"
    exit 0
fi
cp "$tmp/gitleaks.toml" "$dir/gitleaks.toml"
cp "$tmp/LICENSE" "$dir/LICENSE.gitleaks"
cat >"$notice" <<EOF
Detection rules imported as data: the gitleaks default configuration.

source: https://github.com/$repo/blob/$version/config/gitleaks.toml
version: $version
commit: $commit
sha256: $hash
git-blob: $blob
license: MIT, see LICENSE.gitleaks ($(grep -m1 '^Copyright' "$tmp/LICENSE"))
license-sha256: $license_hash
fetched: $(date -u +%Y-%m-%d)

gitleaks.toml is the rule data of gitleaks, a secret scanner, vendored
unmodified. Declass contains none of its code: declass's own detector reads this file
(crates/declass-boundary/src/rules.rs). Update it only with
tools/update-rules.sh <version>, which fetches a release tag, checks the file
against the tag's git blob, shows the rule diff, and rewrites this notice; the
boundary's tests check that the file still has the sha256 above.
EOF
printf '\nvendored %s into %s. Next: run the detection corpus and review the rule diff above:\n' "$version" "$dir"
echo "  cargo test -p declass-boundary --test corpus -- --nocapture"
