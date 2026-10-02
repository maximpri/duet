# Reddit: author brief and technical draft

**Do not paste AI-written copy into r/LocalLLaMA or r/opensource.** Their current rules restrict it; r/LocalLLaMA's narrow language-assistance exception requires transparent disclosure and is not a general exception. Check the project's code eligibility as well. The [research note](RESEARCH.md#reddit-fit-and-rules-first) records the rules and source links. No Reddit post has been made.

Use the facts below to write a personal account from scratch where required. The generic draft is an editorial reference for a community whose current rules permit this kind of assisted post, subject to the author's review and disclosure. It is not a way around another community's restrictions.

## r/LocalLLaMA: technical author brief

Suggested subject: separating a local sensitive-data reader from a frontier coding agent.

- Disclose that you built Duet immediately. State the actual local runtime/model from the run manifest; `omlx-coding` is an alias, not a known weight/version.
- Explain the split: frontier decisions; a local reader without tools; host-enforced filtering, sandbox and audit.
- Show the billing GIF, then link the runnable fixture, recorded requests and canary checker. Identify the LAN transport and fictional data.
- Lead the discussion with what failed: a detector-only diagnostic preview exposed arbitrary CSV fields; the regression records the frontier transport.
- Include the paired benchmark's scope, quality and higher cost. Do not lead with unverifiable “frontier at a fraction of the cost.”
- Ask a question you actually want answered: what should a local model reveal as useful evidence while withholding record values? Invite minimal synthetic attack cases.
- State that hybrid is not fully local and that local-only results depend on the model. The local runtime needs a model endpoint; this is not a bundled model release.

## r/opensource: maintainer author brief

Suggested subject: an open-source coding agent with a verifiable outbound privacy boundary.

Explain the license (**GPL-3.0-or-later**), supported source-build path and how to try a fictional-data task. Explain where contribution would help: platform validation, boundary regressions, better first-run setup, or independent benchmark reproduction. Use the required Promotional flair if the post is eligible. The license alone does not establish posting eligibility, and this community explicitly rejects AI-generated content.

## General technical draft — only where permitted

**Title:** I built a dual-model coding agent with inspectable outbound privacy checks. Our launch demo found a leak.

I’m the author of Duet, a Rust terminal coding agent. Its hybrid mode gives the frontier model the coding decisions and uses a model you control to process sensitive content. The application enforces the boundary with file classification, sandboxed tools, outbound checks and an audit log.

The attached recording is a real run on a tiny billing repository with invented customer records. The task is deliberately easy: fix a status check that includes inactive accounts. The interesting part is the evidence path.

Our first run passed the code tests but exposed customer IDs, reserved-domain emails and amounts in a diagnostic preview. `.invalid` in an email matched an error keyword. The preview removed the recognized name and exposed other fields. We kept the audit, added a regression that inspects the frontier transport, restricted that preview to explicit log files and reran the task. That rerun exposed amounts copied by the local summary; we added a structured-value check and kept that intermediate audit too.

You can inspect the before/after records and rerun the canary check. It checks the complete planted values in a few encodings; it is not a proof against every leak. The new demo uses a self-hosted LAN endpoint with an existing plaintext opt-in and fictional records, so it is not a production transport example.

The frozen six-task benchmark had 18 paired runs per lane: 0 planted canary occurrences with the boundary versus 3,720 without it, and 98.3% versus 97.5% hidden-test success. Cost was 1.36× and time 2.08× the passthrough baseline. The first XL batch missed the quality goal. These are our own development runs.

Hybrid still sends open code and checked context to the frontier. Top clearance removes the frontier and uses the configured local endpoint, with web and command networking disabled. Neither mode protects a compromised host.

Source: https://github.com/maximpri/duet
Evidence: https://github.com/maximpri/duet/blob/main/docs/launch/DEMO.md
Threat model: https://github.com/maximpri/duet/blob/main/SECURITY.md

I’d especially value reproducible synthetic cases that defeat the boundary or show where filtering removes evidence the agent needs. Please use the private reporting route for a live disclosure vulnerability; the release will need that route working before launch.

## Reply facts to have ready

| Question | Accurate answer |
| --- | --- |
| Is everything local? | Hybrid uses a cloud frontier. Sensitive data is handled inside the configured trust boundary. Top clearance uses only the configured local model, which may itself be on a LAN |
| Why not run only a local model? | That is supported. Hybrid aims to retain stronger planning/coding results while controlling disclosure. Quality must be measured, not assumed |
| Is the audit proof nothing leaked? | No. Its integrity and content are separate checks. The failed demo is a concrete example |
| Is it cheaper? | Not in the current paired privacy benchmark. The older cost-saving architecture is a different comparison |
| Is it production approved for banks? | No. The institutional guide is an evaluation starting point |
| Why mention TD Bank? | It is the author's former role and context, not endorsement. It need not be the Reddit headline |

Before submission: recheck rules and eligibility, use a public reviewed revision, inspect the media for real secrets, and make time to participate. Do not mass-post across communities or arrange votes.
