// SPDX-License-Identifier: GPL-3.0-or-later
//! Reproducible review timing, including context for many findings. Run with:
//! cargo test --release -p declass-review --test throughput -- --ignored --nocapture

#[test]
#[ignore = "timing requires an optimized build; reports measurements, not a speed guarantee"]
fn many_findings_with_related_context() {
    use std::{hint::black_box, time::Instant};

    let mut source = String::from("import os\nimport requests\n");
    source.push_str("def clean(value):\n    return value.strip()\n\n");
    for n in 0..24 {
        source.push_str(&format!(
            "def handler_{n}():\n{}    value = clean(request.args['input'])\n    os.system(value)\n\n",
            "    # Context for the security review; parsed but never executed.\n".repeat(24)
        ));
    }
    let expected = declass_review::scan("py", &source, &[]);
    assert_eq!(expected.candidates.len(), 24);
    assert!(!expected.incomplete);
    for _ in 0..3 {
        let started = Instant::now();
        for _ in 0..10 {
            let scan = declass_review::scan("py", black_box(&source), &[]);
            assert_eq!(scan.candidates, expected.candidates);
            assert_eq!(scan.incomplete, expected.incomplete);
            black_box(scan);
        }
        eprintln!(
            "bytes={} candidates={} scans=10 elapsed_ms={:.3}",
            source.len(),
            expected.candidates.len(),
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
}
