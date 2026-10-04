# Installing Duet

## Build from source

The source installer builds the locked dependencies and installs `duet` in
`~/.local/bin`:

```sh
git clone https://github.com/maximpri/duet.git && cd duet && tools/install.sh
```

Rust 1.90 or newer and a C/C++ compiler are required. macOS uses Apple's command
line tools; Linux also needs `bubblewrap` for sandboxed commands. Set
`DUET_INSTALL_DIR` to an absolute path to choose another destination.

## Install a verified binary

The binary installer is prepared for **GitHub Releases published by the owner**.
Use it once a release for your platform and its verified signing identity are
available. It supports macOS and GNU/Linux on ARM64 and x86-64.

Obtain the release signer's public key and identity through an independent
trusted channel. Put the approved line in an allowed-signers file:

```text
<approved-identity> namespaces="duet-release" <approved-SSH-public-key>
```

The placeholders above are not a signing identity. The installer requires your
explicit `--signers` file and never fetches a key from the release server.
From a trusted checkout of this repository, the installation command is:

```sh
tools/install-release.sh 0.1.0 --signers "$HOME/.config/duet/allowed_signers"
```

This downloads the asset for the current operating system and architecture from
`https://github.com/maximpri/duet/releases/download/v0.1.0`. Pin the version in
your deployment scripts. `--url https://your-mirror.example/releases/v0.1.0`
selects a mirror with the same assets; HTTPS is required throughout redirects.

The installer authenticates the archive's SSH signature and SHA-256 checksum,
reads only the expected files from the archive, then verifies the complete inner
release. Only then does it run the binary's version check and atomically replace
`~/.local/bin/duet`. Download, authentication, compatibility or storage failures
leave the existing executable in place.

The matching source archive, license notices, SBOM, build information and signed
checksums remain under `~/.local/share/duet/releases/`. Each installation gets its
own directory, printed on success. `DUET_DATA_DIR` selects another absolute path.
This preserves the source and notices needed to inspect or redistribute that
release. See [licensing and distribution](../LICENSES.md).

The installer requires Bash, curl, OpenSSH with `ssh-keygen -Y`, tar, and either
`shasum` or `sha256sum`. Its accompanying `verify-release.sh` must come from the
same trusted checkout. Linux binary assets target glibc (`*-unknown-linux-gnu`).

## Build unsigned release candidates

From the repository root, run the local validation gate with:

```sh
./cicd.sh check
```

To build macOS and GNU/Linux candidates for ARM64 and x86-64 from one clean,
committed source revision:

```sh
./cicd.sh build --targets all --out /absolute/path/duet-candidates \
  --cache-dir /absolute/path/duet-build-cache
```

`--targets host` is the default when `--targets` is omitted. Keep the output and
cache directories outside the checkout. The full matrix requires a Mac with
Apple's command-line tools for macOS builds and Docker for Linux builds; Linux
uses Rust 1.90 on Debian Bookworm. Cross-architecture version/help checks may use
Rosetta or Docker emulation, which must be available for those targets.
Install both macOS target standard libraries in the same Rust toolchain used by
`cargo` and `rustc`; a target added through rustup is not available to a separate
Homebrew Rust installation. `--targets` also accepts a comma-separated list of
the target triples shown by `./cicd.sh --help`. On Linux, `--targets all` builds
the two Linux targets.

The build runs the full host gate, creates a corresponding-source archive with
locked vendored dependencies, and builds every target from that extracted source.
Each target receives its binary, SBOM, build information, source archive, GPL and
third-party notices, and `SHA256SUMS`. Candidate archives end in
`-unsigned.tar.gz` and have outer checksums; build information records their
unsigned status. Version/help smoke checks are recorded separately from the host
gate. They do not establish full sandbox compatibility on another architecture.

These checksums detect changed bytes but do not authenticate a publisher. The
verified installer rejects unsigned candidates. Once the owner supplies a signing
identity, sign the candidate directory's inner `SHA256SUMS`, verify it with
`tools/verify-release.sh`, then use `tools/package-release.sh` below to produce
signed installer-compatible assets. Keep the matching source and notices with
those assets. Candidate builds neither create signing keys nor publish releases.

## Preparing GitHub release assets

Use an existing owner-controlled signing key and independently maintained
allowed-signers file. Run the release gate and build on each supported host;
`tools/release.sh` builds for that host, rather than cross-compiling implicitly.
For example, on an ARM64 Mac:

```sh
tools/release.sh 0.1.0 --key /secure/path/release-key --out dist/macos-arm64
tools/package-release.sh dist/macos-arm64 \
  --key /secure/path/release-key \
  --signers /secure/path/allowed_signers \
  --out dist/github-macos-arm64
```

`package-release.sh` verifies the input release before wrapping it. Each platform
produces three uniquely named assets, for example:

```text
duet-0.1.0-aarch64-apple-darwin.tar.gz
duet-0.1.0-aarch64-apple-darwin.SHA256SUMS
duet-0.1.0-aarch64-apple-darwin.SHA256SUMS.sig
```

The same naming scheme applies to `x86_64-apple-darwin`,
`aarch64-unknown-linux-gnu` and `x86_64-unknown-linux-gnu`. Each archive includes
its matching source and licenses; no shared asset names collide between hosts.
Build all four platform bundles from the same commit. After collecting them
locally, create the version tag from that build commit. The owner can then create
a draft release and upload the assets:

```sh
gh release create v0.1.0 --repo maximpri/duet --verify-tag --draft --title "Duet 0.1.0" \
  --notes-file /path/to/release-notes.md
gh release upload v0.1.0 --repo maximpri/duet \
  dist/github-macos-arm64/* dist/github-macos-x86_64/* \
  dist/github-linux-arm64/* dist/github-linux-x86_64/*
```

Review the draft and distribute the signing identity through the agreed trusted
channel before publishing it. These scripts do not publish releases or generate
production signing keys.

## Platform checks

The portable Linux check compiles and runs tests using an explicit Rust version
and container architecture. This command checks the minimum supported compiler
and Linux x86-64, using an external directory for downloaded crates and builds:

```sh
tools/release-platform-check.sh --platform linux/amd64 --rust 1.90.0 \
  --target-dir /absolute/path/duet-linux-amd64-rust190
```

Use `linux/arm64` for the ARM64 check. Docker may emulate the selected architecture
when it differs from the host; the output records the platform and compiler.
User-mode emulation can validate compilation and ordinary CLI behavior, but may
reject seccomp filters supported by a native kernel. Validate sandbox behavior
on matching hardware or a full-system guest with that architecture's kernel;
do not count an emulated container's runtime failure as a passing platform check.
The [publication review](PUBLISH_READINESS.md) records the tested environments.
The check first snapshots the selected worktree files so concurrent edits do not
change a running test. The container has namespace privileges for bubblewrap's
runtime tests, with that read-only source snapshot and the explicit writable
cache. The check does not mount the user's home or Docker socket and does not
create or publish release keys.
