# Evaluation report

## Lanes

| Lane | Kind | Runs | Hidden pass rate | Success | Leaks (runs) | Leaks by kind | Sink violations | Mean cost | Mean wall |
|---|---|---|---|---|---|---|---|---|---|
| duet-hybrid | Duet | 4 | 85.0% | 0% | 0 (0) |  | 0 | $0.4477 | 3544s |
| duet-hybrid-compact | Duet | 4 | 80.5% | 0% | 0 (0) |  | 0 | $0.3591 | 4960s |
| duet-hybrid-explore | Duet | 4 | 94.7% | 25% | 0 (0) |  | 0 | $0.5545 | 4464s |
| duet-passthrough | Duet | 4 | 94.2% | 0% | 26838 (4) | email:8856 injection:6 mononym:486 number:231 password:399 person:9834 phone:6482 secret:544 | 0 | $0.3080 | 1729s |

## Duet cost ledger

Means per run, from each run's own summary. Tokens are estimates of frontier input: the size of each tool result summed over every request that carried it, by how it was shown.

| Lane | Runs | Turns | Request tokens | raw | tokenized | summary | answer | bulky | ask_local (questions) | sensitive_data | Denials | Local busy | Frontier in / out |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| duet-hybrid | 4 | 144.5 | 14694.3K | 2919.7K | 5.1K | 542.4K | 98.9K | 145.7K | 5.2 (13.5) | 9.5 | 1.8 | 1162s | $0.3855 / $0.0407 |
| duet-hybrid-compact | 4 | 170.2 | 10215.8K | 3122.2K | 5.9K | 380.2K | 84.6K | 100.9K | 8.8 (23.8) | 7.8 | 2.5 | 2201s | $0.2794 / $0.0496 |
| duet-hybrid-explore | 4 | 172.2 | 19102.2K | 2652.2K | 19.9K | 368.7K | 193.1K | 121.2K | 7.2 (17.8) | 6.2 | 2.8 | 1506s | $0.4798 / $0.0475 |
| duet-passthrough | 4 | 109.8 | 10788.5K | 4889.2K | 0.0K | 0.0K | 0.0K | 0.0K | 0.0 (0.0) | 0.0 | 2.0 | 0s | $0.2800 / $0.0280 |

## Judges

No run was judged.

## Gates

- **duet-hybrid vs duet-passthrough** (n=4): quality undecided (not fully judged); judge not every pair is fully judged; pass rate Δ -9.2 pp, lower bound -22.4 pp (pairs needed 78), behind on 2/2 tasks; cost Δ $+0.1397, upper bound $+0.1820 → FAIL; privacy PASS (leak-rate upper bound 52.7%)
- **duet-hybrid-compact vs duet-passthrough** (n=4): quality undecided (not fully judged); judge not every pair is fully judged; pass rate Δ -13.6 pp, lower bound -26.4 pp (pairs needed 62), behind on 1/2 tasks; cost Δ $+0.0511, upper bound $+0.1121 → FAIL; privacy PASS (leak-rate upper bound 52.7%)
- **duet-hybrid-explore vs duet-passthrough** (n=4): quality undecided (not fully judged); judge not every pair is fully judged; pass rate Δ +0.5 pp, lower bound -1.0 pp (pairs needed 2), behind on 0/2 tasks; cost Δ $+0.2465, upper bound $+0.4208 → FAIL; privacy PASS (leak-rate upper bound 52.7%)

## Cost profile

Per run means. Turns are frontier requests; the context of a turn is what it sent (uncached input + cache reads + cache writes, from the provider's usage in the proxy capture), its mean and 90th percentile over every turn of the runs. Request tokens are the context summed over a run's turns. Turns by cause come from Duet's transcript (sub-agents included): a turn with k tool calls counts 1/k toward each call's cause (reading: read_file, read_raw, list_files, search, diff, git history, web, code_nav; editing: edit_file, write_file, edit_protected, rename, git_commit; commands: run_command; other: finish, delegate, MCP tools, no tool call).

| Lane | Task | Runs | Turns | Context/turn | p90 | Request tokens | Cached | Output | Input share of $ | reading | editing | commands | ask_local | other |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| duet-hybrid | **all** | 4 | 144.5 | 73.5K | 100.7K | 10.62M | 95% | 81.4K | 90% | 33.0 | 30.4 | 76.0 | 3.8 | 1.2 |
| duet-hybrid | X1 | 2 | 167.5 | 75.5K | 102.1K | 12.65M | 95% | 95.6K | 91% | 32.8 | 43.2 | 86.1 | 3.9 | 1.5 |
| duet-hybrid | X2 | 2 | 121.5 | 70.8K | 99.0K | 8.60M | 95% | 67.3K | 90% | 33.3 | 17.5 | 66.0 | 3.7 | 1.0 |
| duet-hybrid-compact | **all** | 4 | 173.2 | 44.6K | 67.5K | 7.73M | 95% | 99.3K | 85% | 50.1 | 24.8 | 90.3 | 6.4 | 1.6 |
| duet-hybrid-compact | X1 | 2 | 165.5 | 45.4K | 65.9K | 7.52M | 96% | 101.2K | 84% | 19.6 | 25.5 | 110.1 | 9.3 | 1.0 |
| duet-hybrid-compact | X2 | 2 | 181.0 | 43.9K | 68.6K | 7.95M | 94% | 97.4K | 86% | 80.7 | 24.0 | 70.6 | 3.5 | 2.2 |
| duet-hybrid-explore | **all** | 4 | 175.2 | 75.8K | 119.6K | 13.29M | 95% | 95.0K | 91% | 31.6 | 31.2 | 105.0 | 5.9 | 1.2 |
| duet-hybrid-explore | X1 | 2 | 169.0 | 77.4K | 112.3K | 13.08M | 95% | 96.5K | 91% | 22.1 | 29.5 | 106.4 | 9.5 | 1.5 |
| duet-hybrid-explore | X2 | 2 | 181.5 | 74.4K | 127.6K | 13.50M | 94% | 93.5K | 91% | 41.0 | 33.0 | 103.6 | 2.4 | 1.0 |
| duet-passthrough | **all** | 4 | 109.8 | 76.1K | 98.7K | 8.35M | 97% | 56.0K | 91% | 11.7 | 20.8 | 75.8 | 0.0 | 1.5 |
| duet-passthrough | X1 | 2 | 128.0 | 73.9K | 95.1K | 9.46M | 97% | 64.6K | 91% | 11.5 | 31.5 | 83.5 | 0.0 | 1.5 |
| duet-passthrough | X2 | 2 | 91.5 | 79.1K | 104.4K | 7.23M | 97% | 47.4K | 91% | 11.8 | 10.0 | 68.2 | 0.0 | 1.5 |

## Price projection

Every run's recorded frontier tokens (uncached input, cache reads, cache writes, output; reasoning tokens are part of every provider's reported output and are charged as output) priced at other list prices. Tokenizer differences are ignored: another model's tokenizer would count the same text differently. The recorded cache split is kept: a provider whose cache writes cost more than input (Anthropic's 5-minute write is 1.25×) would bill part of the uncached input as writes, so such a projection is low by up to a quarter of its uncached-input dollars. Frontier dollars only (local electricity is left out).

Mean frontier dollars per run:

| Lane | Runs | recorded | glm-5.3 | claude-opus-5-5 | gpt-5.5 |
|---|---|---|---|---|---|
| duet-hybrid | 4 | $0.4262 | $3.7549 | $5.8682 | $10.2592 |
| duet-hybrid-compact | 4 | $0.3290 | $2.8975 | $5.0319 | $8.6211 |
| duet-hybrid-explore | 4 | $0.5274 | $4.6448 | $7.1290 | $12.5395 |
| duet-passthrough | 4 | $0.3080 | $2.6974 | $3.7244 | $6.9612 |

Paired ratio against `duet-passthrough` (Σ lane / Σ reference over runs of the same task and seed; 95% bootstrap interval):

| Lane | Pairs | recorded | glm-5.3 | claude-opus-5-5 | gpt-5.5 |
|---|---|---|---|---|---|
| duet-hybrid | 4 | 1.38 [1.28, 1.48] | 1.39 [1.29, 1.49] | 1.58 [1.43, 1.74] | 1.47 [1.36, 1.59] |
| duet-hybrid-compact | 4 | 1.07 [0.75, 1.36] | 1.07 [0.75, 1.38] | 1.35 [0.95, 1.71] | 1.24 [0.87, 1.56] |
| duet-hybrid-explore | 4 | 1.71 [1.07, 2.30] | 1.72 [1.07, 2.32] | 1.91 [1.26, 2.58] | 1.80 [1.17, 2.42] |

Prices (USD per million tokens: input / cache read / cache write / output):

- `glm-5.3`: 1.40 / 0.26 / 1.40 / 4.40 (https://docs.z.ai/guides/overview/pricing, observed 2026-09-23)
- `claude-opus-5-5`: 4.00 / 0.20 / 5.00 / 20.00 (https://platform.claude.com/docs/en/about-claude/pricing, observed 2026-09-23)
- `gpt-5.5`: 5.00 / 0.50 / 5.00 / 30.00 (https://developers.openai.com/api/docs/pricing, observed 2026-09-25)
