// SPDX-License-Identifier: GPL-3.0-or-later
//! What a program that embeds Duet's command line adds to it
//! (ARCHITECTURE.md §13, "Embedding Duet"): the policy layer, audit
//! subscribers and end hooks of `duet_agent::embed`, the product name and
//! version it reports, and checks of its own in `duet doctor`.
//!
//! `duet` itself is [`crate::main_with`] with [`Embedding::default`]: no
//! policy, no hooks, no extra checks, and `duet` as the product. Every
//! command composes runs and sessions the same way with or without them.

use crate::doctor::Check;
use duet_agent::embed::{Hooks, PolicySource};
use duet_config::Config;
use std::path::Path;
use std::sync::Arc;

/// What an embedding program adds to `duet`'s commands. Built with
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
    /// `duet_config::policy`.
    pub fn with_policy(mut self, source: Arc<dyn PolicySource>) -> Self {
        self.policy = Some(source);
        self
    }

    /// Audit subscribers and end hooks for every run and session invocation
    /// (`duet run`, `duet resume`, `duet chat`): subscribers are attached to
    /// the run's audit log as soon as it is opened, before anything is
    /// recorded in it, and end hooks are called once when it ends, in every
    /// terminal state (see `duet_agent::embed`).
    pub fn with_hooks(mut self, hooks: Hooks) -> Self {
        self.hooks = hooks;
        self
    }

    /// A check `duet doctor` (and the TUI's doctor screen) runs after its own.
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

/// The product name and version `--version` and `duet doctor` report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Product {
    /// As shown before the version (`duet`, or the embedding product's name).
    pub name: &'static str,
    pub version: &'static str,
}

impl Default for Product {
    /// Duet itself: `duet` and this crate's version.
    fn default() -> Self {
        Self {
            name: "duet",
            version: CORE_VERSION,
        }
    }
}

/// The version of Duet's crates this program was built with.
pub(crate) const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

impl Product {
    pub fn new(name: &'static str, version: &'static str) -> Self {
        Self { name, version }
    }

    /// Whether this is Duet itself rather than an embedding product.
    pub(crate) fn is_duet(&self) -> bool {
        *self == Self::default()
    }
}

/// What a [`DoctorCheck`] is run with.
#[non_exhaustive]
pub struct DoctorContext<'a> {
    /// The workspace `duet doctor` checks.
    pub workspace: &'a Path,
    /// The configuration as runs see it (with the policy layer), or `None`
    /// when it did not load (Duet's own `config` check then fails and says why).
    pub config: Option<&'a Config>,
    /// `--online`: the check may contact servers. Without it, it must not use
    /// the network.
    pub online: bool,
}

/// A check an embedding program adds to `duet doctor`. It runs after Duet's
/// own checks, on the doctor's thread (it may block briefly), and its lines
/// count towards the exit code like any other: 0 pass, 1 warn, 2 fail.
pub trait DoctorCheck: Send + Sync {
    fn run(&self, ctx: &DoctorContext<'_>) -> Vec<Check>;
}
