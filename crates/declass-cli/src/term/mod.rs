// SPDX-License-Identifier: GPL-3.0-or-later
//! The operator's terminal. How output looks lives in `declass_tui::term`
//! (re-exported here); what stays is `declass run`'s progress on standard error
//! ([`watch`]).

#[cfg(test)]
mod live_tests;
pub(crate) mod watch;

pub(crate) use declass_tui::term::*;
