// SPDX-License-Identifier: GPL-3.0-or-later
//! The `declass` binary: Declass's command line with nothing embedded.

fn main() -> std::process::ExitCode {
    declass_cli::main_with(declass_cli::Embedding::default())
}
