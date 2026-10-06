// SPDX-License-Identifier: GPL-3.0-or-later
//! Measure the initial rules on an external OWASP BenchmarkPython checkout.
//! No benchmark code is bundled or executed. Optional local opinions use the
//! owner's trusted local endpoint; labels and case names are not in prompts.
//! Usage: review_benchmark CORPUS OUTPUT_JSON [OWNER_CONFIG LOCAL_PER_LABEL]

use declass_provider::{ChatProvider, ProviderConfig, Role};
use declass_review::Verdict;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 && args.len() != 5 {
        return Err("expected CORPUS OUTPUT_JSON [OWNER_CONFIG LOCAL_PER_LABEL]".into());
    }
    let corpus = Path::new(&args[1]);
    let local = if args.len() == 5 {
        let cfg = declass_config::Config::load(Path::new(&args[3]), None)?;
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
    let per_label: usize = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(0);
    let anonymize = regex::Regex::new(r"BenchmarkTest\d+")?;
    let csv = std::fs::read_to_string(corpus.join("expectedresults-0.1.csv"))?;
    let start = std::time::Instant::now();
    let mut counts = BTreeMap::<String, [usize; 4]>::new();
    let mut sampled = [0usize; 2];
    let mut rows = Vec::new();
    for line in csv.lines().filter(|s| !s.starts_with('#') && !s.is_empty()) {
        let cols: Vec<_> = line.split(',').collect();
        if cols.len() < 4 || !matches!(cols[1], "sqli" | "cmdi") {
            continue;
        }
        let expected = cols[2] == "true";
        let source =
            std::fs::read_to_string(corpus.join("testcode").join(format!("{}.py", cols[0])))?;
        let scanned = declass_review::scan("py", &source, &[]);
        let relevant = if cols[1] == "sqli" {
            "sql-dynamic-text"
        } else {
            "shell-input"
        };
        let candidates: Vec<_> = scanned
            .candidates
            .into_iter()
            .filter(|c| c.rule == relevant)
            .collect();
        let detected = !candidates.is_empty();
        // TP, FP, FN, TN; score candidates, not just the narrower blockers.
        let index = match (expected, detected) {
            (true, true) => 0,
            (false, true) => 1,
            (true, false) => 2,
            (false, false) => 3,
        };
        counts.entry(cols[1].to_owned()).or_default()[index] += 1;
        let mut opinion = None;
        let mut seconds = 0.0;
        if let Some(local) = &local
            && sampled[usize::from(expected)] < per_label
            && let Some(c) = candidates.first()
        {
            sampled[usize::from(expected)] += 1;
            let mut c = c.clone();
            c.context = anonymize.replace_all(&c.context, "handler").into_owned();
            let clock = std::time::Instant::now();
            opinion = Some(match local.review_candidate(&c).await {
                Ok(j) => json!({"retained":j.verdict != Verdict::Unlikely, "judgment":j}),
                Err(e) => json!({"retained":true, "error":e}),
            });
            seconds = clock.elapsed().as_secs_f64();
            eprintln!(
                "reviewed sample {} ({seconds:.1}s)",
                sampled.iter().sum::<usize>()
            );
        }
        rows.push(
            json!({"case":cols[0],"category":cols[1],"expected":expected,"candidate":detected,
            "incomplete":scanned.incomplete,"opinion":opinion,"local_seconds":seconds}),
        );
    }
    let report = json!({"revision":declass_review::REVISION,"counts_tp_fp_fn_tn":counts,
        "seconds":start.elapsed().as_secs_f64(),"local_stats":local.as_ref().map(|l| l.take_stats()),"cases":rows});
    std::fs::write(&args[2], serde_json::to_vec_pretty(&report)?)?;
    println!("{}", report["counts_tp_fp_fn_tn"]);
    Ok(())
}
