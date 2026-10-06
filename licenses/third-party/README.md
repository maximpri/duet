<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Supplementary third-party license notices

These files supplement license texts omitted from three published Cargo packages.
The package identities and upstream revisions come from their `Cargo.toml` and
`.cargo_vcs_info.json` files in the locked, vendored source. Retrieved October 4,
2026; the upstream texts are copied without changes or Declass license headers.
They retain their upstream terms. This README is GPL-3.0-or-later.

| Package and version | Upstream revision | Retained notice |
| --- | --- | --- |
| `objc2-core-foundation 0.3.2`, `objc2-io-kit 0.3.2` | [`7b1abfd750a2cacaea71d6a56ecfb83cb7de560b`](https://github.com/madsmtm/objc2/tree/7b1abfd750a2cacaea71d6a56ecfb83cb7de560b) | [Upstream grant and SDK caveat](objc2-frameworks-0.3.2/LICENSE.md), [Apache-2.0 text](objc2-frameworks-0.3.2/LICENSE-APACHE-2.0.txt) |
| `tree-sitter-typescript 0.23.2` | [`f975a621f4e7f532fe322e13c4f79495e0a7b2e7`](https://github.com/tree-sitter/tree-sitter-typescript/tree/f975a621f4e7f532fe322e13c4f79495e0a7b2e7) | [MIT license and copyright](tree-sitter-typescript-0.23.2/LICENSE) |

The objc2 upstream grant offers these two framework crates under
`Zlib OR Apache-2.0 OR MIT`. We include the full Apache-2.0 text for that offered
option; this does not change the upstream alternatives. The same upstream notice
also records the project's uncertainty about Apple SDK-derived bindings. It is
preserved verbatim, not resolved or replaced by this supplement. No standalone
`NOTICE` file is present in that upstream revision.

## Exact sources and hashes

| Local file | Download source | SHA-256 |
| --- | --- | --- |
| `objc2-frameworks-0.3.2/LICENSE.md` | [Pinned upstream file](https://raw.githubusercontent.com/madsmtm/objc2/7b1abfd750a2cacaea71d6a56ecfb83cb7de560b/LICENSE.md) | `7f976f7e9cb2d87df7230606feb932c3f21ac0e664045a775b600046ff850c54` |
| `objc2-frameworks-0.3.2/LICENSE-APACHE-2.0.txt` | [Apache Software Foundation](https://www.apache.org/licenses/LICENSE-2.0.txt), the license linked by the upstream grant | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |
| `tree-sitter-typescript-0.23.2/LICENSE` | [Pinned upstream file](https://raw.githubusercontent.com/tree-sitter/tree-sitter-typescript/f975a621f4e7f532fe322e13c4f79495e0a7b2e7/LICENSE) | `49bf33cf78ef5897e4e161ce1517df7de1ae5042a65b6bcfd44401e0fc606559` |

Verify the unmodified texts from this directory:

```sh
shasum -a 256 -c SHA256SUMS
```

`r-efi 5.3.0` and `r-efi 6.0.0` already retain the complete MIT grant and
copyright notices in their vendored `AUTHORS` files; no supplement is needed
for that offered license option. A filename-only search for `LICENSE` misses
these notices.

Keep this directory in the corresponding-source archive alongside `vendor/`.
Do not insert files into Cargo's vendored packages or rewrite their checksum
manifests. See [Licensing and distribution](../../LICENSES.md).
