# Final vendored source validation

The [manifest](validation.json) records archive commit `1e4f2e4`, hashes,
commands, compiler and compared files. The archive has no AppleDouble metadata
or external filesystem links. All 2,517 tracked entries match that commit;
19,550 dependency files across 320 packages match Cargo's checksums. License
texts and upstream notices are retained with the sources.

An optimized `--frozen` offline build from `ae66b4a` passed using an initially
empty Cargo home and an existing external build cache. `duet --version` passed.
All 404 build inputs and 19,870 vendored files in the final archive match that
successful extraction. The later revision only changes documentation, evidence
and exclusion of a local experiment-storage link; the manifest lists the delta.
The Cargo home contains no downloaded registry sources, cache or index.

- [Offline build log](normalized-offline-build.log.gz)
- [Final archive packaging log](normalized-source-package.log.gz)
- [Pinned checkout log](normalized-checkout.log.gz)

These are source/build validation artifacts, not a signed production release.
The full archive and raw records remain in the owner's external validation
workspace, identified by their hashes. Regenerate matching source from the
clean release commit when producing production binary assets.
