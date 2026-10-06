// SPDX-License-Identifier: GPL-3.0-or-later
//! Release tooling. [`sbom`] builds a CycloneDX 1.5 software bill of materials
//! for one workspace package from `cargo metadata` and `Cargo.lock`, without
//! network access or third-party plugins.

pub mod sbom;
