# Duet: nine tasks, 27 paired seeds

Duet's mean counted hidden-test score was **89.26%**, compared with **96.54%** for the same frontier model with the privacy boundary disabled. The hybrid lane had **zero planted private-value matches in 1,124 captured frontier requests**.

The October 4, 2026 verification covers **54 outcomes and 27 task/seed pairs**. It retains **53 native records and grader records**, plus one externally stopped hybrid outcome counted zero. Two fresh full artifact audits agreed before the publication verdict was written; ROOT and a separate sub-agent independently checked the scores and accounting.

[Per-case report](report.md) · [Machine-readable results](report.json) · [Publication verdict](audit-verdict.json) · [Provenance](provenance.json) · [Checksums](SHA256SUMS)

| Result | Hybrid | Passthrough |
| --- | ---: | ---: |
| Counted case outcomes | 27 | 27 |
| Mean counted hidden-test score | **89.26%** | **96.54%** |
| Successful native terminals | 26 | 27 |
| Grader success outcomes | 14 | 15 |
| Product failure outcomes | 1 | 0 |
| Native records / grader records | 26 / 26 | 27 / 27 |
| Captured frontier requests | 1,124 | 1,328 |
| Observed planted-value matches | 0 | 35,809 |
| Cases with workspace sink checks | 27 | 27 |
| Observed sink violations | 0 | 0 |

Scores give each of the 27 cases in a lane equal weight. They are not the fraction of all assertions passed. The report preserves partial test results and includes every selected outcome. A native completion, a grader record and a fully successful grade are separate facts. M1 hybrid seed 1 has a grader record with no observed hidden-test results following a compilation failure; its zero score does not mean 12 assertions executed.

## The externally stopped outcome

X1 hybrid seed 3 exhausted the reviewed conservative deadline while the original native process remained live and no native terminal artifacts were present. A separately recorded intervention froze scheduling and ended that same attempt. It counts as a zero-score product failure and was not rerun. No native `run.json`, summary or grade was manufactured; this is an externally enforced outcome, not a native-reported timeout. All 53 actual native records have no recorded execution or grading timeout.

The pending frontier request's unknown-usage reservation remains charged. The final X2 hybrid seed 3 then completed normally, passed 48 of 55 hidden tests and settled all 135 of its frontier requests.

## All-attempt accounting

| Accounting item | Amount |
| --- | ---: |
| Settled usage: 2,483 requests | $23.10446950 |
| Retained unknown usage: two requests | $0.44564480 |
| **Conservative total across all attempts** | **$23.55011430** |
| Owner's aggregate frontier allowance | $50.00 |
| Active allocations at final verification | $0.00 |

The retained reservations are original request 88 and X1 request 39, each $0.22282240. Every original, serial, parallel, recovery and final-attempt journal remains included. Interrupted and excluded attempts still cost money; all retained capture bodies, including bodies without request-status records, were inventoried. No reservation was reset or refunded without complete captured response usage. These figures are conservative journal accounting, not a provider invoice, subscription-quota measurement or measured electricity cost.

## Build, collection and measurement scope

The benchmark application is frozen at [`b9eb511d9e96c2d4fbb9a816ba6b6b0470c8dfa4`](https://github.com/maximpri/duet/tree/b9eb511d9e96c2d4fbb9a816ba6b6b0470c8dfa4). Later packaging and documentation work does not change that application. The [provenance file](provenance.json) binds the source archive, binaries, input manifest, reports and audit verdict without exposing private paths, endpoints, prompts or captures.

- Nine tasks were evaluated with three paired seeds and two lanes. The frontier model and model setup were held constant; hosted model aliases and the local endpoint do not pin immutable weights.
- Collection combined serial execution, at most one hybrid and two passthrough workers, and separately reviewed continuation controllers. Recorded timings are not a controlled lane-latency comparison.
- A naturally completed orphaned case was recovered without rerunning it. Its unavailable parent wait status and incomplete earlier descendant history remain disclosed. Later quiescence checks do not reconstruct that lost history. The final X2 harness has an actual parent wait record.
- The configured native whole-run limit also covers pending provider calls through cooperative runtime timers. The observed X1 overrun does not establish its runtime cause or prove a timeout fix. The [security-review brief](../../SECURITY_REVIEW_BRIEF.md) retains an offline reproduction question.
- The traffic checks measure literal planted values and workspace sink matches. Zero observations do not establish general semantic secrecy, provider retention behavior or deployment approval.
- No fresh quality judges ran. These are project-run mechanical evaluations and independent artifact checks within the project, not a commissioned external security assessment or a general frontier-quality parity claim.
- Final verification acquired every recorded lease and found no remaining or uncertain recorded processes, writable artifact handles or active allocations before accepting the packet.

The [original accounting-stop packet](../release-hardening-2026-10-03/benchmark/README.md) remains unchanged. [Earlier evaluations](../../VALUE_EVIDENCE.md) retain their own builds and methods. [Continuation procedure](../../BOUNDED_BENCHMARKS.md) explains why the evaluation's external controllers are not a shipped automatic-resume feature.

To check the exported bytes from this directory:

```sh
shasum -a 256 -c SHA256SUMS
```
