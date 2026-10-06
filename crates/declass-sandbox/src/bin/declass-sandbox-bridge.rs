// SPDX-License-Identifier: GPL-3.0-or-later
//! The egress bridge helper as a program of its own, for this crate's tests
//! (a release runs it as `declass __sandbox-bridge`).

fn main() {
    std::process::exit(declass_sandbox::bridge::main(
        std::env::args_os().skip(1).collect(),
    ));
}
