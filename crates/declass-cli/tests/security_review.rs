// SPDX-License-Identifier: GPL-3.0-or-later
//! Finish-time rules through the real loop and gate; no live provider.
mod privacy;
use declass_agent::Terminal;
use privacy::{Fixture, Local, Options, Step};
use serde_json::{Value, json};

const BAD: &str = "import requests\nrequests.get(url, verify=False)\n";

#[tokio::test(flavor = "multi_thread")]
async fn a_finish_is_blocked_then_the_fixed_change_completes() {
    let mut f = Fixture::with(
        "review-fix",
        "Update client",
        Local::Cooperative,
        Options {
            no_local: true,
            ..Options::default()
        },
    );
    f.cfg.review.enabled = true;
    f.cfg.review.block_high = true;
    // Existing unsafe code is outside the change and must not block.
    std::fs::write(f.ws.join("old.py"), BAD).unwrap();
    f.script(vec![
        Step::Call("write_file", json!({"path":"client.py", "content":BAD})),
        Step::Call("finish", json!({"summary":"done"})),
        Step::Call(
            "write_file",
            json!({"path":"client.py", "content":BAD.replace("False", "True")}),
        ),
        Step::Call("finish", json!({"summary":"fixed"})),
    ]);
    let terminal = f.run().await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    assert_eq!(f.unscripted(), 0);
    let bodies: Vec<_> = f
        .bodies()
        .iter()
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .collect();
    assert!(bodies[2].contains("tls-verification-disabled"));
    assert!(bodies[2].contains("task is not complete"));
    let report: Value =
        serde_json::from_slice(&std::fs::read(f.run_dir.join("security-review.json")).unwrap())
            .unwrap();
    assert_eq!(report["blocked"], false);
    assert_eq!(report["findings"], json!([]));
    f.assert_no_leak(&declass_boundary::testing::canary::Canaries::new(
        privacy::planted(),
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn defaults_leave_runs_unchanged_and_missing_local_does_not_veto_rules() {
    for enabled in [false, true] {
        let mut f = Fixture::with(
            "review-default",
            "Update client",
            Local::Cooperative,
            Options {
                no_local: true,
                ..Options::default()
            },
        );
        f.cfg.review.enabled = enabled;
        f.cfg.review.block_high = true;
        f.script(vec![
            Step::Call("write_file", json!({"path":"client.py", "content":BAD})),
            Step::Call("finish", json!({"summary":"done"})),
            Step::Call("finish", json!({"summary":"done"})),
        ]);
        let terminal = f.run().await;
        assert_eq!(
            matches!(terminal, Terminal::Failed { .. }),
            enabled,
            "{terminal:?}"
        );
        assert_eq!(f.run_dir.join("security-review.json").exists(), enabled);
    }
}
