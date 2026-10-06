# Licensing and distribution

Declass Core is licensed under **GPL-3.0-or-later**, as declared in the workspace
manifest and source headers. [LICENSE](LICENSE) contains the full GPL version 3
text; [NOTICE](NOTICE) states the grant and warranty disclaimer. This applies to
Declass's original code and documentation unless a file or fixture specifies other
terms. Third-party material keeps its own terms.

## Included third-party material

| Material | License and provenance |
| --- | --- |
| Embedded gitleaks detection rules | [MIT license](crates/declass-boundary/rules/LICENSE.gitleaks), [pinned source and hash](crates/declass-boundary/rules/NOTICE) |
| sqlparser evaluation starter and reference overlay | [Apache-2.0](tasks/X1-sql-gateway/starter/LICENSE.TXT), [source and modifications](tasks/X1-sql-gateway/NOTES.md) |
| log crate in the sqlparser fixture | [MIT](tasks/X1-sql-gateway/starter/vendor/log/LICENSE-MIT) or [Apache-2.0](tasks/X1-sql-gateway/starter/vendor/log/LICENSE-APACHE) |
| JSONata evaluation starter and reference overlay | [MIT](tasks/X2-partner-exports/starter/LICENSE), [source and modifications](tasks/X2-partner-exports/NOTES.md) |
| Cargo dependencies | License expressions in their manifests; published source and notices retained in the release source archive's `vendor/` directory |
| objc2-core-foundation 0.3.2 and objc2-io-kit 0.3.2 | [Supplementary upstream grant, Apache-2.0 text and provenance](licenses/third-party/README.md); upstream also offers Zlib or MIT and records an Apple SDK-derived bindings caveat |
| tree-sitter-typescript 0.23.2 | [Supplementary pinned MIT license](licenses/third-party/tree-sitter-typescript-0.23.2/LICENSE), [source and checksum](licenses/third-party/README.md) |

The corresponding-source archive also includes tracked `licenses/third-party/`
supplements when a published Cargo package omits license texts. The supplements
preserve exact upstream notices and identify their package versions, source
revisions and hashes without altering vendored files or checksum manifests.
The `r-efi` packages retain their MIT grant and copyrights in `AUTHORS`.

Do not replace upstream notices with Declass's SPDX header. The evaluation fixtures
are not linked into the Declass executable. Model weights, model services and
external tools installed separately are governed by their respective terms.

`cargo deny --locked check licenses` checks dependency license policy. This does
not replace preserving notices or supplying corresponding source. `MPL-2.0` is
file-level copyleft, not a permissive license; retain its source and notices if
an allowed dependency uses it.

## Binary releases

Run `./cicd.sh build` from a clean, committed checkout for unsigned candidates,
or use `tools/release.sh` for optional SSH-signed builds. Alongside the binary,
SBOM, build information and checksums, both paths package:

- `LICENSE` and `NOTICE`;
- `LICENSES.md`, the embedded rules' MIT license and provenance;
- `declass-<version>-source.tar.gz`: the tracked source at the build commit,
  build/install scripts and complete locked Cargo dependency sources, including
  their upstream license and notice files;
- `SOURCE.txt`: the matching source archive, commit and offline build command.

Publish all these files together at the same download location and retain the
source archive for as long as you offer the binary. The archive is the source
for that exact release; a moving branch or a Cargo.lock file alone is not a
substitute. It vendors the main Cargo workspace's dependencies, including build
and test dependencies for all platforms. The separate optional `fuzz/` workspace
is not part of the binary build and has its own dependency setup.

The release SBOM records the binary's normal/build dependency closure. License
texts are supplied in the accompanying source archive through `vendor/` and
`licenses/third-party/`, not merely as SPDX identifiers in the SBOM. Any separate
binary package or installer must keep
the license/notices available and clearly identify where recipients can obtain
the matching source archive at no further charge.

When distributing modified versions, preserve notices, identify your changes and
their dates, and provide the corresponding source under the applicable GPL
terms. Distribution in a GPL-defined User Product can also require Installation
Information that lets recipients install and run modified versions.

These release steps implement the repository's distribution procedure. The
[GPL text, especially sections 4–6](https://www.gnu.org/licenses/gpl-3.0.html),
controls the obligations for a particular distribution; GNU also provides
[licensing guidance](https://www.gnu.org/licenses/gpl-howto.html).
