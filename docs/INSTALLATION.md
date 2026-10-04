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

## Install the latest GitHub release

The installer selects the latest published release and the correct asset for
macOS or GNU/Linux, on ARM64 or x86-64. Preview releases are unsigned:

```sh
curl --proto '=https' --proto-redir '=https' -fsSL https://raw.githubusercontent.com/maximpri/duet/main/install.sh | bash -s -- --allow-unsigned
```

There are no published GitHub releases yet. This command is ready for the first
release; until its assets are uploaded, use the source installer above or a local
candidate. Re-running it installs the then-latest release. To pin a version, add
`0.1.0` after `--allow-unsigned`.

`--allow-unsigned` explicitly selects the `-unsigned` assets. The installer checks
the archive and every payload file against SHA-256 checksums, checks the platform
and reported version, and replaces the executable atomically. Checksums detect
changed bytes; they do not establish who published them. This path trusts GitHub
and its HTTPS connection for the downloaded script and release files. It never
falls back to unsigned installation after a failed signed installation.

Duet is installed to `~/.local/bin/duet`; set `DUET_INSTALL_DIR` to another absolute
path if needed. Matching GPL source, licenses, SBOM and build records are retained
under `~/.local/share/duet/releases/` (`DUET_DATA_DIR` overrides this). Existing
release records remain available. The installer does not use `sudo` or edit shell
profiles. Add `~/.local/bin` to `PATH` if your shell does not already include it.

For inspection before execution, download `install.sh`, read it, and run
`bash install.sh --allow-unsigned`. The script contains its verification helpers;
it does not download additional scripts. Maintainers regenerate it with
`python3 tools/build-bootstrap.py` after changing the helpers.

The installer needs Bash, curl, tar, and either `shasum` or `sha256sum`.
Signature verification additionally needs OpenSSH with `ssh-keygen -Y`.
Linux assets target glibc (`*-unknown-linux-gnu`); command isolation also requires
bubblewrap, user namespaces and seccomp support. No Rust compiler is needed.

## Install a verified binary

Signing is optional. To authenticate signed releases, omit `--allow-unsigned` and
configure a trusted signer first. The installer then requires both the outer
archive signature and the complete inner release signature to pass before it
runs downloaded code.

Obtain the release signer's public key and identity through an independent
trusted channel. Put the approved line in an allowed-signers file:

```text
<approved-identity> namespaces="duet-release" <approved-SSH-public-key>
```

The placeholders above are not a signing identity. Save the approved line to
`~/.config/duet/allowed_signers` (or `$DUET_CONFIG_HOME/allowed_signers`). The
installer never downloads the trusted key with the release. For the latest
signed release:

```sh
curl --proto '=https' --proto-redir '=https' -fsSL https://raw.githubusercontent.com/maximpri/duet/main/install.sh | bash
```

The bootstrap script still comes from GitHub over HTTPS; SSH signatures
authenticate the release payload against your separately trusted key. From a
trusted checkout, pin a version and select a signers file with:

```sh
tools/install-release.sh 0.1.0 --signers "$HOME/.config/duet/allowed_signers"
```

This downloads the asset for the current operating system and architecture from
`https://github.com/maximpri/duet/releases/download/v0.1.0`. Pin the version in
your deployment scripts. `--url https://your-mirror.example/releases/v0.1.0`
selects a mirror with the same assets; HTTPS is required throughout redirects.

Both installation modes retain the source and notices needed to inspect or
redistribute the release. See [licensing and distribution](../LICENSES.md).

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
`-unsigned.tar.gz` and have outer checksums; macOS also receives unsigned `.dmg`
images. Build information records their unsigned status. Version/help smoke checks are recorded separately from the host
gate. They do not establish full sandbox compatibility on another architecture.

These checksums detect changed bytes but do not authenticate a publisher. The
installer requires `--allow-unsigned` to use candidates. Upload the files in
`assets/` to the matching GitHub release (`v0.1.0` for version `0.1.0`) to make
them available to the remote installer. Keep the matching source and notices
with those assets. Candidate builds neither create signing keys nor publish
releases. Signing can be added later using the procedure below.

## macOS disk images

The macOS build also produces an architecture-specific `.dmg`. Duet is a terminal
application: open the image and run `Install Duet.command` to install it for your
user account. The unsigned preview installer asks you to confirm that choice.
It checks the bundled files, retains the source and notices, and installs to
`~/.local/bin` without administrator access. It does not change Gatekeeper settings.
Unsigned images are not Apple-notarized and macOS may block them.

To package an existing candidate on a Mac:

```sh
tools/package-dmg.sh /absolute/path/candidates/releases/aarch64-apple-darwin \
  --unsigned --out /absolute/path/duet-0.1.0-aarch64-apple-darwin-unsigned.dmg
```

Use `x86_64-apple-darwin` for Intel Macs. Keep the generated checksum beside the
image when distributing it. Each image contains the complete release payload,
including its matching vendored source, license notices and installation scripts.

## Preparing GitHub release assets

For unsigned previews, upload `assets/` from `cicd.sh build` as-is. Use a tag of
`vVERSION` matching the workspace and asset version. Include the archives, disk
images and their checksums; the remote installer downloads the archive for its
platform. Signing is optional and is described next.

### Optional SSH signatures

A dedicated Ed25519 release key can be generated locally; it does not require an
Apple account or certificate. Keep the private key outside the repository. Only
the public key, fingerprint and allowed-signers identity should be distributed.

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
For an SSH-authenticated macOS image, pass the signed release directory to
`tools/package-dmg.sh` with `--key`, `--signers` and `--out /path/to/IMAGE.dmg`.
It verifies the payload and signs the image's separate checksum manifest.
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

### Optional Apple signing and notarization

SSH signatures authenticate Duet releases on both macOS and Linux. Apple signing
is a separate step for macOS distribution. It needs:

- An Apple Developer Program membership and a **Developer ID Application**
  certificate with its private key in the signing Mac's keychain. An Apple
  Development certificate is not the distribution identity. Developer ID
  Installer is only needed for a `.pkg`; Duet currently ships a `.dmg`.
- Xcode or its command-line tools, including `codesign`, `notarytool` and `stapler`.
- Notarization credentials: an Apple account, team ID and app-specific password,
  or an App Store Connect API key. Store credentials in Keychain, not this repo.

[Apple's Developer ID guide](https://developer.apple.com/developer-id/) explains
membership and certificate setup. Its
[packaging guide](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution)
describes signing binaries and disk images. Save notarization credentials using
an interactive prompt so the password is not written into shell history:

```sh
xcrun notarytool store-credentials duet-notary --apple-id YOUR_APPLE_ID --team-id YOUR_TEAM_ID
```

The [notarytool guide](https://developer.apple.com/documentation/technotes/tn3147-migrating-to-the-latest-notarization-tool)
covers both credential types. The release order matters:

1. Start from a fresh copy of the matching unsigned candidate. Sign its executable
   with the Developer ID Application identity, hardened runtime and secure
   timestamp (`codesign --force --options runtime --timestamp --sign IDENTITY BINARY`).
   Verify its signature and test Duet's command sandbox with that signed binary.
2. Record the signing step in `BUILDINFO.txt`, regenerate the inner `SHA256SUMS`
   over the final payload, and optionally sign that manifest with the SSH release
   key. Apple signing changes the binary bytes, so its earlier checksum is stale.
3. Package the final payload. Sign the `.dmg` with the Developer ID Application
   identity and a timestamp. Submit it with
   `xcrun notarytool submit IMAGE.dmg --keychain-profile duet-notary --wait`.
4. After an **Accepted** result, run `xcrun stapler staple IMAGE.dmg` and
   `xcrun stapler validate IMAGE.dmg`, then test installation on a separate Mac.
5. Generate the outer image checksum and optional SSH signature **last**, after
   signing and stapling have finished changing the image. Publish those final bytes.

The current scripts build unsigned images or SSH-authenticated release assets;
they do not perform Apple signing or notarization automatically. No production
SSH key is bundled. Keep it outside the repository and distribute only its public
key and fingerprint through a channel recipients already trust.

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
