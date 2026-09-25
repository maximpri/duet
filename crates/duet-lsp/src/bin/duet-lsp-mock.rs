// SPDX-License-Identifier: GPL-3.0-or-later
//! The scripted test server of `duet_lsp::mock` as a program on stdio.

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = duet_lsp::mock::MockOptions::from_args(&args);
    match duet_lsp::mock::serve(tokio::io::stdin(), tokio::io::stdout(), opts, 1).await {
        duet_lsp::mock::End::Crash => std::process::exit(3),
        _ => std::process::exit(0),
    }
}
