# Earning “frontier results at a fraction of the cost”

**Status: proposed experiment, not a result.** The current [paired privacy benchmark](../VALUE_EVIDENCE.md) costs 1.36× the same frontier without the boundary. An earlier public cost-saving article describes a different architecture. Neither a cheap single task nor zero configured local token prices establishes a saving.

## Separate the two questions

1. **What does the privacy boundary cost?** Compare hybrid and passthrough with the same frontier, task, seed, agent, checks, budget and starting repository. Keep the existing lane as the control. Use only synthetic private data in passthrough.
2. **Can the complete workflow replace an expensive frontier workflow?** Compare a named premium frontier baseline with a named Duet configuration on identical sealed tasks. Report model changes as part of the intervention. This measures the combined workflow, not the boundary's isolated effect.

Local-only is a third, separately labelled lane. It must meet its own quality bar before its cost can be compared as an equivalent solution. A less capable answer at a lower token price is not equivalent work.

## Freeze the protocol before running

- Use at least 20 distinct held-out tasks across bug fixes, tests, refactors, integration changes and protected-data work; include difficult multi-file tasks. Pair three seeds per task and randomize execution order. This is a proposed minimum, not a power calculation.
- Freeze starting revisions, task prompts, hidden tests, canaries, model/provider identifiers, sampling parameters, policy, context limits, tool availability and time/spend caps. Keep a public manifest with hashes.
- Record local runtime, model weights and quantization, hardware, warm/cold state and model licences. A friendly model alias is insufficient for reproduction.
- Record every attempt, timeout, retry and manual intervention. Score exhausted and invalid runs as failures. Do not select the cheapest successful seed after seeing the results.
- Use independent request capture for both frontier and local endpoints; keep synthetic canaries distinct from public fixture words. Include literal/encoded extraction, derived files, hostile instructions and semantic leakage review. Automated matching cannot cover every disclosure class.
- Grade hidden tests and human review blind to lane. Keep correctness, security regressions and task completion separate. Pair uncertainty calculations by task, not by treating every assertion as an independent sample.

Existing assets to extend: [suite definition](../DOGFOOD_SUITE.md), [benchmark harness](../../crates/duet-evals/src/benchmark.rs), [acceptance ledger](../ACCEPTANCE.md), [privacy transport scenarios](../../crates/duet-cli/tests/privacy_scenarios.rs). This document does not claim these proposed lanes are already configured or run.

## Price successful work, including failures

Report both aggregate task cost and cost per accepted task:

```text
total cost = billed model charges, including retries and cache charges
           + measured local energy cost
           + declared hardware amortization allocation
           + attributable hosting/licence costs

cost per accepted task = total spend on all attempted tasks / accepted tasks
saving = 1 - (Duet cost per accepted task / baseline cost per accepted task)
```

Show operator review time separately and provide a total-cost scenario with an explicit hourly rate. Report token-price estimates separately from reconciled invoices. Meter local busy/idle energy where possible; otherwise publish the wattage assumptions and sensitivity range. Existing hardware still has capacity and operating costs. A zero-success lane has no finite cost-per-success value.

## Proposed publication gates

These are project targets to agree and freeze, not industry standards or achieved results.

| Claim | Minimum proposed gate |
| --- | --- |
| Similar quality on the named suite | Lower end of a paired 95% confidence interval for task success difference above −5 percentage points; no critical security regression; report task-stratified results |
| A fraction of the cost | At most 25% of baseline cost per accepted task while meeting the quality gate; uncertainty interval must also support the advertised reduction |
| Boundary holds on the tested attacks | No observed prohibited canaries, reviewed semantic disclosures or bypasses in the declared corpus; publish corpus, denominators and known coverage gaps |
| Practical to use | Median and p95 completion/review time reported, including failures; reviewer-defined latency target fixed before running |

Do not generalize beyond the tested tasks, models and deployment. Have an outside maintainer reproduce a subset before making a broad comparative claim. Publish a miss as a result, with the next hypothesis.

## Optimization order

Measure where money and time go before adding orchestration. Prior work found that deleting reasoning and aggressive local compaction could lose quality; [retain those failed experiments](../VALUE_EVIDENCE.md#where-cost-work-stands).

1. Separate unavoidable frontier context from repeated context; measure real provider cache usage.
2. Prefer deterministic schema extraction for questions that do not need a model. Check whether delayed local digests reduce latency without losing necessary evidence.
3. Experiment with bounded local work on tasks with machine-checkable outcomes. Keep the same policy and verification gates; record recovery cost when the frontier must intervene.
4. Evaluate context changes on the hard tasks first. Promote only configurations that pass the frozen quality and privacy gates.

Until these results exist, the launch claim is **frontier coding with an inspectable privacy boundary**, with current costs shown alongside quality.
