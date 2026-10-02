// SPDX-License-Identifier: GPL-3.0-or-later
//! Freeze an independently selected benchmark subset before inspecting source.
//! No snippets are executed. The manifest pins every selected source digest.
use serde_json::json;
use std::path::Path;

fn freeze(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(&args[2]);
    let mut groups = std::collections::BTreeMap::<(String, bool), Vec<String>>::new();
    let categories = [
        "codeinj",
        "deserialization",
        "hash",
        "pathtraver",
        "xss",
        "xxe",
    ];
    for line in std::fs::read_to_string(root.join("expectedresults-0.1.csv"))?.lines() {
        let c: Vec<_> = line.split(',').map(str::trim).collect();
        if c.len() < 4 || !categories.contains(&c[1]) {
            continue;
        }
        groups
            .entry((c[1].into(), c[2] == "true"))
            .or_default()
            .push(c[0].into());
    }
    let mut cases = Vec::new();
    for ((category, expected), mut names) in groups {
        names.sort_by_key(|n| {
            duet_fs::sha256_hex(format!("review-independent-2026-09-30:{n}").as_bytes())
        });
        for name in names.into_iter().take(4) {
            let source = std::fs::read(root.join("testcode").join(format!("{name}.py")))?;
            cases.push(json!({"case":name,"category":category,"expected":expected,"sha256":duet_fs::sha256_hex(&source)}));
        }
    }
    std::fs::write(
        &args[3],
        serde_json::to_vec_pretty(
            &json!({"selection":"four per category/label, sorted by fixed salted SHA-256 before source inspection","cases":cases}),
        )?,
    )?;
    println!("frozen {} cases", cases.len());
    Ok(())
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
    use duet_boundary::{
        OutboundGate, engine::Engine, policy::Policy, review::SecondReviewer, view::Presenter,
    };
    use duet_provider::{ChatProvider, ProviderConfig, Role};
    use duet_review::Verdict;
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 4 && args[1] == "freeze" {
        return freeze(&args);
    }
    if !matches!(args.len(), 5 | 6) || args[1] != "run" {
        return Err(
            "expected freeze CORPUS MANIFEST, or run CORPUS MANIFEST OUTPUT [OWNER_CONFIG]".into(),
        );
    }
    let corpus = Path::new(&args[2]);
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(&args[3])?)?;
    let state = Path::new(&args[4]).with_extension("state");
    duet_fs::private::ensure_private_dir(&state)?;
    let engine = Engine::open(&state, Policy::default(), None)?;
    let mut local = None;
    let mut second = None;
    let mut model_names = json!({});
    if let Some(owner) = args.get(5) {
        let cfg = duet_config::Config::load(Path::new(owner), None)?;
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
        local = Some(duet_boundary::local::LocalReader::new(
            ChatProvider::with_reqwest(pc)?,
        ));
        let mut pc = ProviderConfig::new(
            &cfg.str("frontier.base_url")?,
            &cfg.str("frontier.model")?,
            Role::Frontier,
        );
        pc.max_attempts = Some(1);
        pc.dialect = duet_provider::Dialect::parse(&cfg.str("frontier.dialect")?)
            .ok_or("unknown dialect")?;
        pc.api_key_env = Some(cfg.str("frontier.api_key_env")?).filter(|s| !s.is_empty());
        let (filter, check) = engine.outbound();
        let gate = OutboundGate::new(duet_boundary::audit::AuditLog::open(
            &state.join("audit.jsonl"),
        )?)
        .with_filter(filter)
        .with_check(check);
        let reviewer = SecondReviewer::new(gate.wrap(ChatProvider::with_reqwest(pc)?), 0.50)?;
        engine.set_review_second(&reviewer)?;
        second = Some(reviewer);
        model_names = json!({"local":cfg.str("local.model")?,"frontier":cfg.str("frontier.model")?,
            "frontier_max_output_tokens":duet_boundary::review::MAX_OUTPUT_TOKENS,
            "frontier_reasoning_effort":if cfg.str("frontier.model")?.to_ascii_lowercase().starts_with("glm-5.3") { "low" } else { "provider default" }});
    }
    let anonymize = regex::Regex::new(r"BenchmarkTest\d+")?;
    let start = std::time::Instant::now();
    let mut counts = [[0usize; 4]; 4];
    let mut rows = vec![];
    for case in manifest["cases"].as_array().ok_or("missing frozen cases")? {
        let name = case["case"].as_str().ok_or("missing case")?;
        let source = std::fs::read_to_string(corpus.join("testcode").join(format!("{name}.py")))?;
        if duet_fs::sha256_hex(source.as_bytes()) != case["sha256"] {
            return Err("frozen benchmark source changed".into());
        }
        let category = case["category"].as_str().unwrap_or("");
        let rule = match category {
            "codeinj" => "code-input",
            "deserialization" => "unsafe-deserialization",
            "hash" => "weak-crypto",
            "pathtraver" => "path-input",
            "xss" => "html-input",
            "xxe" => "xml-input",
            _ => return Err("unknown frozen category".into()),
        };
        let scanned = duet_review::scan("py", &source, &[]);
        let candidates: Vec<_> = scanned
            .candidates
            .into_iter()
            .filter(|c| c.rule == rule)
            .collect();
        let expected = case["expected"].as_bool().ok_or("missing label")?;
        let mut detected = [!candidates.is_empty(), false, false, false];
        let mut opinions = vec![];
        for mut candidate in candidates {
            candidate.context = anonymize
                .replace_all(&candidate.context, "handler")
                .into_owned();
            let local_opinion = if let Some(local) = &local {
                Some(local.review_candidate(&candidate).await)
            } else {
                None
            };
            // Opinions use the bounded function/helper context, with no labels,
            // case names, working conversation or tools. Every send is gated.
            let diff = json!({"before":"","after":candidate.context}).to_string();
            let frontier_opinion = if second.is_some() {
                engine.review_second(&candidate, &["candidate.py".into()], &diff, 0.50)
            } else {
                None
            };
            let retain = |j: &Option<Result<duet_review::Judgment, String>>| !matches!(j, Some(Ok(j)) if j.verdict == Verdict::Unlikely);
            let a = retain(&local_opinion);
            let b = retain(&frontier_opinion);
            detected[1] |= a;
            detected[2] |= b;
            detected[3] |= a || b;
            opinions.push(json!({"local":local_opinion,"frontier":frontier_opinion}));
        }
        for (lane, detected) in detected.into_iter().enumerate() {
            score(&mut counts[lane], expected, detected);
        }
        rows.push(json!({"case":name,"category":category,"expected":expected,"incomplete":scanned.incomplete,"detected":detected,"opinions":opinions}));
        let report = json!({"revision":duet_review::REVISION,"models":model_names,"manifest_sha256":duet_fs::sha256_hex(&std::fs::read(&args[3])?),
            "lanes":["rules","rules_local","rules_frontier","both_must_dismiss"],"counts_tp_fp_fn_tn":counts,"seconds":start.elapsed().as_secs_f64(),"cases":rows});
        duet_fs::private::write_private(Path::new(&args[4]), &serde_json::to_vec_pretty(&report)?)?;
        eprintln!(
            "measured {} frozen cases ({:.1}s)",
            rows.len(),
            start.elapsed().as_secs_f64()
        );
    }
    let report = json!({"revision":duet_review::REVISION,"models":model_names,"manifest_sha256":duet_fs::sha256_hex(&std::fs::read(&args[3])?),
        "lanes":["rules","rules_local","rules_frontier","both_must_dismiss"],"counts_tp_fp_fn_tn":counts,"seconds":start.elapsed().as_secs_f64(),
        "local_stats":local.as_ref().map(|l| l.take_stats()),"frontier_stats":second.as_ref().map(|s|s.take_stats()),"cases":rows});
    duet_fs::private::write_private(Path::new(&args[4]), &serde_json::to_vec_pretty(&report)?)?;
    println!("{}", report["counts_tp_fp_fn_tn"]);
    Ok(())
}
