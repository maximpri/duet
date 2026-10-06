// SPDX-License-Identifier: GPL-3.0-or-later
//! Watching the frontier's responses as they stream, for the operator's
//! terminal (the `declass` workspace, `declass run`).
//!
//! The interface sets a tap for the work it drives ([`observe`]); every
//! request the [`crate::GatedFrontier`] sends within it streams to the tap
//! as well. Only gated requests are watched: the local model's reading is
//! never shown this way. A sub-agent's loop runs [`quiet`], so only the
//! loop the operator talks to is shown. Watching changes nothing that is
//! sent or recorded.

pub use declass_provider::live::{StreamEvent, StreamTap};
use std::future::Future;
use std::sync::Arc;

tokio::task_local! {
    static TAP: Option<Arc<dyn StreamTap>>;
}

/// Runs `work` with the frontier's responses streaming to `tap`.
pub async fn observe<F: Future>(tap: Arc<dyn StreamTap>, work: F) -> F::Output {
    TAP.scope(Some(tap), work).await
}

/// Runs `work` without a tap, whatever the caller watches.
pub async fn quiet<F: Future>(work: F) -> F::Output {
    TAP.scope(None, work).await
}

/// The tap of the work being run, if it is watched.
pub(crate) fn current() -> Option<Arc<dyn StreamTap>> {
    TAP.try_with(Clone::clone).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Nothing;
    impl StreamTap for Nothing {
        fn event(&self, _: StreamEvent<'_>) {}
    }

    #[tokio::test]
    async fn a_tap_holds_inside_observe_only_and_quiet_hides_it() {
        assert!(current().is_none());
        observe(Arc::new(Nothing), async {
            assert!(current().is_some());
            quiet(async { assert!(current().is_none()) }).await;
            assert!(current().is_some());
        })
        .await;
        assert!(current().is_none());
    }
}
