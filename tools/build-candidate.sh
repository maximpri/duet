#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Internal offline build worker for cicd.sh; run inside its frozen source tree.
set -euo pipefail
[ "$#" -eq 6 ] || { echo 'usage: build-candidate.sh SOURCE TARGET VERSION COMMIT EPOCH OUTPUT' >&2; exit 2; }
source_dir=$1 target=$2 version=$3 commit=$4 epoch=$5 out=$6
cd "$source_dir"
case "${CARGO_TARGET_DIR:-}" in
    /*) target_dir=$CARGO_TARGET_DIR ;;
    *) echo 'CARGO_TARGET_DIR must be absolute' >&2; exit 1 ;;
esac
export SOURCE_DATE_EPOCH="$epoch"
unset CARGO_BUILD_TARGET
host=$(rustc -vV | sed -n 's/^host: //p')
printf 'Build host: %s\nTarget: %s\n' "$host" "$target"
rustc -V
cargo -V
DUET_RELEASE_BUILD=1 cargo build --release --frozen -p duet-cli --bin duet --target "$target"
binary="duet-$version-$target"
cp "$target_dir/$target/release/duet" "$out/$binary"
actual=$("$out/$binary" --version)
[ "$actual" = "duet $version" ] || { echo "unexpected binary version: $actual" >&2; exit 1; }
"$out/$binary" --help >/dev/null
printf 'Smoke checks passed: %s --version; --help\n' "$binary"
cargo run --quiet --frozen -p duet-release --bin duet-sbom -- \
    --target "$target" --out "$out/duet-$version.cdx.json"
cat >"$out/BUILDINFO.txt" <<INFO
duet $version
commit $commit
target $target
$(rustc -V)
$(cargo -V)
source_date_epoch $epoch
status unsigned candidate
build_host $host
environment ${DUET_BUILD_ENVIRONMENT:-local host}
smoke_checks --version and --help passed; no cross-target sandbox qualification
built with: DUET_RELEASE_BUILD=1 cargo build --release --frozen -p duet-cli --bin duet --target $target
INFO
