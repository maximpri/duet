// SPDX-License-Identifier: GPL-3.0-or-later
//! The `duet` binary: Duet's command line with nothing embedded.

fn main() -> std::process::ExitCode {
    duet_cli::main_with(duet_cli::Embedding::default())
}
