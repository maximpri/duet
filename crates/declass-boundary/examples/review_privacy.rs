// SPDX-License-Identifier: GPL-3.0-or-later
//! Authored development corpus, not a holdout. Never executes the snippets.
//! Usage: review_privacy OUTPUT_JSON [OWNER_CONFIG [DOGFOOD_RUST_SOURCE]]
//! Local prompts contain source and rule only. Reports use the production
//! privacy/IP filters, with separate forced-echo probes of those filters.
use declass_boundary::engine::Engine;
use declass_boundary::policy::{Policy, StructureSettings};
use declass_boundary::view::{Explored, Presenter, Source};
use declass_provider::{ChatProvider, ProviderConfig, Role};
use declass_review::Verdict;
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Deserialize)]
struct Case {
    id: String,
    ext: String,
    expected: bool,
    source: String,
}

fn score(counts: &mut [usize; 4], expected: bool, detected: bool) {
    counts[match (expected, detected) {
        (true, true) => 0,
        (false, true) => 1,
        (true, false) => 2,
        (false, false) => 3,
    }] += 1;
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if !matches!(args.len(), 2..=4) {
        return Err("expected OUTPUT_JSON [OWNER_CONFIG [DOGFOOD_RUST_SOURCE]]".into());
    }
    let local = if let Some(owner) = args.get(2) {
        let cfg = declass_config::Config::load(Path::new(owner), None)?;
        let mut pc = ProviderConfig::new(
            &cfg.str("local.base_url")?,
            &cfg.str("local.model")?,
            Role::Local {
                allowlist: cfg.list("local.allowlist")?,
                allow_plaintext: cfg.bool("local.allow_plaintext")?,
            },
        );
        pc.max_attempts = Some(1);
        pc.api_key_env = Some(cfg.str("local.api_key_env")?).filter(|s| !s.is_empty());
        Some(declass_boundary::local::LocalReader::new(
            ChatProvider::with_reqwest(pc)?,
        ))
    } else {
        None
    };
    let d = tempfile::tempdir()?;
    let policy = Policy {
        sealed: vec!["candidate.*".into()],
        sensitive_globs: vec!["data/**".into()],
        detect_pii: true,
        detect_secrets: true,
        structure: StructureSettings {
            views: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let e = Engine::open(d.path(), policy, local)?;
    let ws = d.path().join("ws");
    std::fs::create_dir_all(ws.join("data"))?;
    std::fs::write(
        ws.join("data/customer.csv"),
        "birth_date,card\n1994-11-07,5293761582049377\n",
    )?;
    e.prime(&ws, &["data/customer.csv".into()], "review changes");
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../tests/fixtures/review_privacy.json"))?;
    let fields = e.review_fields();
    if !fields.contains(&"birth_date".into()) || e.review_value_spans("'1994-11-07'").is_empty() {
        return Err("privacy fixture did not seed its schema and known value".into());
    }
    let mut rules = [0; 4];
    let mut combined = [0; 4];
    let mut rows = Vec::new();
    let start = std::time::Instant::now();
    let mut leaks = 0;
    let mut unavailable = 0;
    let base = args.get(3).map(std::fs::read_to_string).transpose()?;
    for case in cases {
        if base.is_some() && case.ext != "rs" {
            continue;
        }
        let mut source = case
            .source
            .replace("handler", "hidden_review_transform_7ca91f");
        if let Some(base) = &base {
            source = format!("{base}\n{source}");
        }
        let after = declass_review::scan_with_values(
            &case.ext,
            &source,
            &fields,
            &e.review_value_spans(&source),
        );
        let before = base
            .as_ref()
            .map(|b| {
                declass_review::scan_with_values(&case.ext, b, &fields, &e.review_value_spans(b))
            })
            .unwrap_or_default();
        let scan = declass_review::introduced(before, after);
        let candidates: Vec<_> = scan
            .candidates
            .into_iter()
            .filter(|c| c.rule == "sensitive-sink")
            .collect();
        score(&mut rules, case.expected, !candidates.is_empty());
        let mut retained = false;
        let mut opinions = Vec::new();
        let source_context = Source::Explore {
            question: "Review security and suggest a safe fix".into(),
            read: Explored(Arc::new(vec![(
                Some(PathBuf::from(format!("candidate.{}", case.ext))),
                source.clone(),
            )])),
        };
        let canaries = [
            "1994-11-07",
            "review-person@harbor-ops.net",
            "hidden_review_transform_7ca91f",
        ];
        let call_start = std::time::Instant::now();
        for candidate in &candidates {
            match e.review_candidate(candidate) {
                Some(Ok(j)) => {
                    retained |= j.verdict != Verdict::Unlikely;
                    let shown = e.present(
                        &source_context,
                        format!("{}\n{}", j.explanation, j.fix).as_bytes(),
                    );
                    leaks += usize::from(canaries.iter().any(|s| shown.contains(s)));
                    opinions.push(json!({"verdict":j.verdict,"report":shown}));
                }
                _ => {
                    retained = true;
                    unavailable += 1;
                }
            }
        }
        score(&mut combined, case.expected, retained);
        // Probe even when the model correctly avoids quoting. This is separate
        // from model accuracy and never alters the opinion's confusion matrix.
        let probe = e.present(
            &source_context,
            format!("Customer born 1994-11-07.\n{source}").as_bytes(),
        );
        let probe_leak = canaries.iter().any(|s| probe.contains(s));
        leaks += usize::from(probe_leak);
        rows.push(
            json!({"case":case.id,"expected":case.expected,"candidate":!candidates.is_empty(),
            "incomplete":scan.incomplete,"retained":retained,"opinions":opinions,
            "local_seconds":call_start.elapsed().as_secs_f64(),"forced_echo_leak":probe_leak}),
        );
        eprintln!("reviewed privacy case {}", rows.len());
    }
    let report = json!({"revision":declass_review::REVISION,"counts_tp_fp_fn_tn":rules,
        "retained_counts_tp_fp_fn_tn":combined,"local_enabled":args.len()>=3,
        "dogfood_source_sha256":base.as_ref().map(|b| declass_fs::sha256_hex(b.as_bytes())),
        "frontier_eligible":false,"four_lane_counts":[rules,combined,rules,combined],
        "local_unavailable":unavailable,"canary_leaks":leaks,
        "local_stats":e.take_local_stats(),"seconds":start.elapsed().as_secs_f64(),"cases":rows});
    std::fs::write(&args[1], serde_json::to_vec_pretty(&report)?)?;
    println!(
        "rules={} retained={} canary_leaks={leaks}",
        report["counts_tp_fp_fn_tn"], report["retained_counts_tp_fp_fn_tn"]
    );
    if leaks > 0 {
        return Err("privacy report canary check failed".into());
    }
    Ok(())
}
