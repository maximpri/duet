# Evaluation report

## Lanes

| Lane | Kind | Runs | Hidden pass rate | Success | Leaks (runs) | Leaks by kind | Sink violations | Mean cost | Mean wall |
|---|---|---|---|---|---|---|---|---|---|
| duet-hybrid | Duet | 18 | 98.3% | 78% | 0 (0) |  | 0 | $0.0256 | 539s |
| duet-hybrid-nolocal | Duet | 18 | 85.3% | 56% | 0 (0) |  | 0 | $0.0432 | 530s |
| duet-passthrough | Duet | 18 | 97.5% | 67% | 3720 (18) | email:1284 injection:100 mononym:63 number:463 password:41 person:1519 phone:168 secret:82 | 0 | $0.0189 | 259s |

## Duet cost ledger

Means per run, from each run's own summary. Tokens are estimates of frontier input: the size of each tool result summed over every request that carried it, by how it was shown.

| Lane | Runs | Turns | Request tokens | raw | tokenized | summary | answer | bulky | ask_local (questions) | sensitive_data | Denials | Local busy | Frontier in / out |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| duet-hybrid | 18 | 19.8 | 438.2K | 54.0K | 0.4K | 41.3K | 18.2K | 1.9K | 3.1 (10.8) | 2.0 | 0.1 | 207s | $0.0157 / $0.0066 |
| duet-hybrid-nolocal | 18 | 29.3 | 1005.5K | 78.8K | 0.6K | 107.9K | 0.0K | 1.5K | 0.1 (0.2) | 8.8 | 1.1 | 0s | $0.0312 / $0.0119 |
| duet-passthrough | 18 | 19.8 | 369.7K | 78.8K | 0.0K | 0.0K | 0.0K | 0.0K | 0.0 (0.0) | 0.0 | 0.1 | 0s | $0.0132 / $0.0058 |

## Judges

2 judges: **claude** (anthropic family; claude-cli `claude-opus-5-5`, CLI 2.1.283 (Claude Code); 54 runs), **codex** (openai family; codex-cli `gpt-5.5`, CLI codex-cli 0.157.1; 54 runs). Gates use each run's mean over its judges; a run counts only when both claude and codex scored it.

| Lane | Family | Runs | Fully judged | claude /30 | codex /30 | Mean /30 | Self-judged |
|---|---|---|---|---|---|---|---|
| duet-hybrid | zhipu | 18 | 18 | 19.2 | 20.9 | 20.1 | no |
| duet-hybrid-nolocal | zhipu | 18 | 18 | 19.0 | 20.2 | 19.6 | no |
| duet-passthrough | zhipu | 18 | 18 | 18.6 | 21.3 | 19.9 | no |

Self-judged: the judge shares the lane's model family and may favour it; each judge's column shows the scores separately.

Agreement, claude vs codex (54 runs scored by both): mean absolute difference 3.49/30, Pearson correlation 0.10.

## Gates

- **duet-hybrid vs duet-passthrough** (n=18): quality PASS; judge Δ +0.2/30, lower bound -0.5 → PASS; pass rate Δ +0.8 pp, lower bound -0.3 pp (pairs needed 4), behind on 1/6 tasks; cost Δ $+0.0067, upper bound $+0.0118 → FAIL; privacy PASS (leak-rate upper bound 15.3%)
- **duet-hybrid vs duet-hybrid-nolocal** (n=18): quality PASS; judge Δ +0.5/30, lower bound -0.5 → PASS; pass rate Δ +13.1 pp, lower bound +3.9 pp (pairs needed 176), behind on 0/6 tasks; cost Δ $-0.0175, upper bound $-0.0087 → PASS; privacy PASS (leak-rate upper bound 15.3%)

## Cost profile

Per run means. Turns are frontier requests; the context of a turn is what it sent (uncached input + cache reads + cache writes, from the provider's usage in the proxy capture), its mean and 90th percentile over every turn of the runs. Request tokens are the context summed over a run's turns. Turns by cause come from Duet's transcript (sub-agents included): a turn with k tool calls counts 1/k toward each call's cause (reading: read_file, read_raw, list_files, search, diff, git history, web, code_nav; editing: edit_file, write_file, edit_protected, rename, git_commit; commands: run_command; other: finish, delegate, MCP tools, no tool call).

| Lane | Task | Runs | Turns | Context/turn | p90 | Request tokens | Cached | Output | Input share of $ | reading | editing | commands | ask_local | other |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| duet-hybrid | **all** | 18 | 19.8 | 18.8K | 32.7K | 372.3K | 90% | 13.3K | 70% | 3.6 | 7.2 | 6.0 | 1.9 | 1.1 |
| duet-hybrid | L1 | 3 | 27.0 | 24.2K | 34.2K | 652.8K | 88% | 18.6K | 76% | 4.9 | 10.0 | 7.8 | 3.2 | 1.0 |
| duet-hybrid | M1 | 3 | 29.0 | 22.4K | 33.9K | 650.1K | 93% | 19.9K | 71% | 3.6 | 11.3 | 10.0 | 2.4 | 1.7 |
| duet-hybrid | M2 | 3 | 15.3 | 12.1K | 17.1K | 186.1K | 89% | 8.0K | 67% | 3.0 | 2.7 | 5.3 | 3.3 | 1.0 |
| duet-hybrid | M3 | 3 | 15.7 | 16.0K | 24.7K | 250.6K | 88% | 10.2K | 68% | 3.7 | 6.0 | 3.7 | 1.2 | 1.2 |
| duet-hybrid | S1 | 3 | 15.3 | 10.0K | 16.8K | 153.5K | 86% | 5.3K | 73% | 3.5 | 6.7 | 4.2 | 0.0 | 1.0 |
| duet-hybrid | S2 | 3 | 16.7 | 20.4K | 33.7K | 340.5K | 90% | 17.5K | 62% | 3.1 | 6.7 | 4.8 | 1.1 | 1.0 |
| duet-hybrid-nolocal | **all** | 18 | 29.3 | 28.5K | 51.1K | 834.2K | 94% | 23.9K | 72% | 4.8 | 8.6 | 14.6 | 0.1 | 1.2 |
| duet-hybrid-nolocal | L1 | 3 | 47.0 | 35.8K | 57.7K | 1.68M | 95% | 35.6K | 78% | 6.5 | 15.8 | 23.7 | 0.0 | 1.0 |
| duet-hybrid-nolocal | M1 | 3 | 47.0 | 34.5K | 54.3K | 1.62M | 95% | 40.4K | 74% | 6.9 | 14.5 | 23.8 | 0.0 | 1.8 |
| duet-hybrid-nolocal | M2 | 3 | 16.0 | 16.2K | 26.9K | 259.8K | 88% | 15.2K | 60% | 1.9 | 2.7 | 10.3 | 0.0 | 1.1 |
| duet-hybrid-nolocal | M3 | 3 | 26.3 | 25.8K | 50.0K | 678.2K | 93% | 20.6K | 72% | 4.2 | 9.0 | 11.8 | 0.0 | 1.3 |
| duet-hybrid-nolocal | S1 | 3 | 14.0 | 8.5K | 13.2K | 119.3K | 90% | 5.5K | 65% | 4.5 | 4.2 | 4.0 | 0.3 | 1.0 |
| duet-hybrid-nolocal | S2 | 3 | 25.3 | 25.5K | 40.7K | 646.5K | 92% | 25.8K | 66% | 4.7 | 5.7 | 14.0 | 0.0 | 1.0 |
| duet-passthrough | **all** | 18 | 19.9 | 16.5K | 30.3K | 327.0K | 91% | 11.5K | 70% | 4.8 | 7.8 | 6.1 | 0.0 | 1.1 |
| duet-passthrough | L1 | 3 | 21.0 | 19.7K | 50.9K | 413.9K | 90% | 11.6K | 75% | 5.3 | 8.7 | 5.7 | 0.0 | 1.3 |
| duet-passthrough | M1 | 3 | 29.3 | 19.1K | 26.4K | 560.9K | 95% | 19.0K | 68% | 4.7 | 13.0 | 10.7 | 0.0 | 1.0 |
| duet-passthrough | M2 | 3 | 12.3 | 6.5K | 10.1K | 80.4K | 90% | 4.6K | 60% | 4.7 | 2.3 | 4.3 | 0.0 | 1.0 |
| duet-passthrough | M3 | 3 | 19.0 | 16.9K | 25.0K | 321.0K | 90% | 13.0K | 67% | 4.0 | 7.0 | 6.7 | 0.0 | 1.3 |
| duet-passthrough | S1 | 3 | 15.7 | 7.9K | 11.8K | 120.4K | 92% | 5.0K | 66% | 3.8 | 6.2 | 4.3 | 0.0 | 1.0 |
| duet-passthrough | S2 | 3 | 22.0 | 21.2K | 35.7K | 465.4K | 89% | 15.7K | 72% | 6.5 | 9.7 | 4.8 | 0.0 | 1.0 |
