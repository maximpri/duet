// SPDX-License-Identifier: GPL-3.0-or-later
//! Portable extension metadata. Discovery never executes code or opens the network.

pub mod skills;

pub use skills::{Catalog, Document, Origin, Root, Skill};
