// SPDX-License-Identifier: GPL-3.0-or-later
//! Handle-relative workspace I/O, atomic durable writes, locks and the .declass path registry.

pub mod error;
pub mod fault;
pub mod host;
pub mod lock;
pub mod ops;
pub mod path;
pub mod pinned;
pub mod private;
pub mod registry;

pub use error::FsError;
pub use ops::{
    Precondition, WriteReceipt, atomic_write, read_file, read_optional, remove_file, restore,
    sha256_hex,
};
pub use path::{is_reserved, normalize_relative, writable_relative};
