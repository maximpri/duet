// SPDX-License-Identifier: GPL-3.0-or-later
//! What a program that embeds Declass's command line adds to it
//! (ARCHITECTURE.md §13, "Embedding Declass"): the policy layer, audit
//! subscribers and end hooks of `declass_agent::embed`, the product name and
//! version it reports, and checks of its own in `declass doctor`.
//!
//! `declass` itself is [`crate::main_with`] with [`Embedding::default`]: no
//! policy, no hooks, no extra checks, and `declass` as the product. Every
//! command composes runs and sessions the same way with or without them.

use crate::doctor::Check;
use declass_agent::embed::{Hooks, PolicySource};
use declass_config::Config;
use std::path::Path;
use std::sync::Arc;

/// What an embedding program adds to `declass`'s commands. Built with
/// [`Embedding::new`] (or [`Embedding::default`] for none) and the methods
/// below; cheap to clone.
#[derive(Clone, Default)]
pub struct Embedding {
    pub(crate) product: Product,
    pub(crate) policy: Option<Arc<dyn PolicySource>>,
    pub(crate) hooks: Hooks,
    pub(crate) doctor: Vec<Arc<dyn DoctorCheck>>,
}

impl std::fmt::Debug for Embedding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Embedding")
            .field("product", &self.product)
            .field("policy", &self.policy.is_some())
            .field("hooks", &self.hooks)
            .field("doctor", &self.doctor.len())
            .finish()
    }
}

impl Embedding {
    /// An embedding that reports itself as `product` and adds nothing else yet.
    pub fn new(product: Product) -> Self {
        Self {
            product,
            ..Self::default()
        }
    }

    /// The policy layer above the owner's and the project's configuration,
    /// for every command that loads configuration (runs, sessions, `config`,
    /// `doctor`, the TUI). A source that fails makes those commands fail: see
    /// `declass_config::policy`.
    pub fn with_policy(mut self, source: Arc<dyn PolicySource>) -> Self {
        self.policy = Some(source);
        self
    }

    /// Audit subscribers and end hooks for every run and session invocation
    /// (`declass run`, `declass resume`, a `declass` session): subscribers are attached to
    /// the run's audit log as soon as it is opened, before anything is
    /// recorded in it, and end hooks are called once when it ends, in every
    /// terminal state (see `declass_agent::embed`).
    pub fn with_hooks(mut self, hooks: Hooks) -> Self {
        self.hooks = hooks;
        self
    }

    /// A check `declass doctor` (and the TUI's doctor screen) runs after its own.
    pub fn with_doctor_check(mut self, check: Arc<dyn DoctorCheck>) -> Self {
        self.doctor.push(check);
        self
    }

    pub(crate) fn product(&self) -> &Product {
        &self.product
    }

    pub(crate) fn policy_source(&self) -> Option<&dyn PolicySource> {
        self.policy.as_deref()
    }

    pub(crate) fn hooks(&self) -> &Hooks {
        &self.hooks
    }

    pub(crate) fn doctor_checks(&self) -> &[Arc<dyn DoctorCheck>] {
        &self.doctor
    }
}

/// The product name and version `--version` and `declass doctor` report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Product {
    /// As shown before the version (`declass`, or the embedding product's name).
    pub name: &'static str,
    pub version: &'static str,
}

impl Default for Product {
    /// Declass itself: `declass` and this crate's version.
    fn default() -> Self {
        Self {
            name: "declass",
            version: CORE_VERSION,
        }
    }
}

/// The version of Declass's crates this program was built with.
pub(crate) const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

impl Product {
    pub fn new(name: &'static str, version: &'static str) -> Self {
        Self { name, version }
    }

    /// Whether this is Declass itself rather than an embedding product.
    pub(crate) fn is_declass(&self) -> bool {
        *self == Self::default()
    }
}

/// What a [`DoctorCheck`] is run with.
#[non_exhaustive]
pub struct DoctorContext<'a> {
    /// The workspace `declass doctor` checks.
    pub workspace: &'a Path,
    /// The configuration as runs see it (with the policy layer), or `None`
    /// when it did not load (Declass's own `config` check then fails and says why).
    pub config: Option<&'a Config>,
    /// `--online`: the check may contact servers. Without it, it must not use
    /// the network.
    pub online: bool,
}

/// A check an embedding program adds to `declass doctor`. It runs after Declass's
/// own checks, on the doctor's thread (it may block briefly), and its lines
/// count towards the exit code like any other: 0 pass, 1 warn, 2 fail.
pub trait DoctorCheck: Send + Sync {
    fn run(&self, ctx: &DoctorContext<'_>) -> Vec<Check>;
}
