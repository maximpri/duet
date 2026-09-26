// SPDX-License-Identifier: GPL-3.0-or-later
//! The egress bridge helper as a program of its own, for this crate's tests
//! (a release runs it as `duet __sandbox-bridge`).

fn main() {
    std::process::exit(duet_sandbox::bridge::main(
        std::env::args_os().skip(1).collect(),
    ));
}
