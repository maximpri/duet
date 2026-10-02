# Campaign: earn 10,000 GitHub stars through adoption

**Recommendation:** run a 90-day evidence-led launch, followed by a measured 3–6 month adoption campaign if the early targets hold. The goal is 10,000 stars, not a promised outcome or a substitute for usage. Position Duet as **secure by design, with a boundary developers and security teams can inspect**.

## Baseline and required scale

Owner-reported mlxtop result: **50,000 LinkedIn impressions / 90 stars**. That is 0.18 stars per 100 impressions as a rough cross-platform ratio, not measured attribution or a visitor conversion rate. At the same ratio, 10,000 stars would correspond to approximately **5.56 million impressions**. The star target is about **111×** the mlxtop baseline; repeating that launch ten times would yield about 900 stars at that ratio.

Record Duet's actual star count on day 0. All milestone counts below are **net new campaign stars**; subtract existing stars when translating the 10,000-total goal. Use these scenarios to size the work, not to promise performance:

| Scenario | Qualified repository visitors | Visitor-to-star assumption | Implied stars |
| --- | ---: | ---: | ---: |
| Low conversion | 200,000 | 5% | 10,000 |
| Planning case | 125,000 | 8% | 10,000 |
| Strong fit | 83,334 | 12% | approximately 10,000 |

These are hypotheses, not industry benchmarks. At 1%, 2% or 4% impression-to-repository-visit assumptions, the planning case would need 12.5M, 6.25M or 3.125M impressions. Returning visitors and multiple channels make simple attribution unreliable. Use the measured first month to replace these assumptions.

## Who should discover it first

1. **Local-model developers:** they already control an inference server and can try a dual-model workflow. Offer a reproducible task, endpoint instructions and honest quality/cost results.
2. **Security-minded developers and platform engineers:** they need a traceable answer to what an agent sends, and useful OS-level controls. Offer the canary suite and an audit walkthrough.
3. **Banking and public-sector technology/security leaders:** use Maxim's former Senior Managing Architect at TD Bank role as context. Offer a concrete evaluation packet, not a claim of institutional approval.

Lead broad developer content with the working product. Use the banking background in founder content and institutional conversations, without logos or implied endorsement.

## Before a broad launch

These are substantive release tasks; this documentation branch does not mark them complete:

- **Close disclosure findings.** Merge the dogfood regression fix only after review; retain the before/after evidence. Run a new independent red-team pass and representative quality gates after the tightened preview behavior.
- **Provide a real private vulnerability-reporting route.** `SECURITY.md` and `docs/security.txt` still contain placeholder contact information. The owner must select and verify a contact or private reporting route before a security-led public push.
- **Make installation dependable.** Provide reviewed, signed/checksummed artifacts and platform smoke tests before claiming one-command binary installation. Today the source build is the supported path.
- **Make a first success reproducible.** Test the billing fixture on a clean machine, specify the actual local model and transport, and give a useful failure message when it is unavailable.
- **Publish the evidence on the same public revision as the README.** Check every media and audit link after merge; a local worktree link is not a launch URL.
- **Make scope obvious.** Hybrid sends open code and checked context. Self-hosted LAN is not same-device. The current cost-saving goal is unmet. Distinguish test doubles from live model evidence.

## Ninety days of useful releases

Dates are relative to a release-ready day 0. These are stretch checkpoints with decision gates, not an assertion that this cadence will produce the counts.

| Phase | Work and content | Distribution | Target / decision |
| --- | --- | --- | --- |
| Days −14 to −1 | Clean-install trials, disclosure fix review, 20–30 second clip, proof packet, reporting contact | Invite a small set of interested testers personally when the owner chooses to do so | 10 external testers; 8 complete the fixture without assistance before broad distribution |
| Days 0–7 | Founder introduction + live TUI; same-day architecture answers; working source release | LinkedIn primary post, one appropriate human-authored Reddit contribution; Show HN once genuinely tryable | 500 new stars; 30 successful trials; respond to substantive questions within one working day |
| Days 8–21 | Publish the discovered leak, reproduction and fix; audit verification walkthrough | LinkedIn follow-up; a relevant technical community discussion under its rules | 1,000 cumulative new stars; 10 external reproductions; resolve top setup blockers |
| Days 22–45 | Protected-code demo; local-only task; measured false positives and latency | Collaborations with local-runtime maintainers and technical creators who choose to evaluate it | 3,000 cumulative; 3 independently authored evaluations; first meaningful outside contributions |
| Days 46–60 | New paired privacy/quality/cost batch, including failures; deployable audit export design | Engineering article, demo session, appropriate banking/platform meetups | 5,000 cumulative; 3 synthetic institutional pilots if readiness permits |
| Days 61–90 | Substantive product improvements from feedback; external assessment results if available | A new release story grounded in new evidence; user-authored examples | 10,000 cumulative stretch target; assess 30-day returning users and maintainer load |

If the 10,000-star goal is not reached, continue only channels that generate trials, return usage or useful contributions. A slower trusted tool is a better outcome than a brief spike without adoption.

## Weekly publishing rhythm

Repository setup for the launch: suggested About text is **“A Rust coding agent with frontier reasoning, a local sensitive-data reader, sandboxed tools and verifiable outbound audits.”** Candidate topics: `coding-agent`, `local-llm`, `privacy`, `security`, `rust`, `tui`, `audit-log`. Set these only when the reviewed revision is public. Pin the demo/evidence link and keep the install path above the full feature catalogue. These are suggested metadata changes; the remote repository has not been edited.

- **One product demonstration:** one task, one security boundary, one visible result. Use actual terminal footage, fictional data and linked audit evidence.
- **One engineering finding:** a leak class, a false positive, a latency issue or a failed experiment. Show the regression and scope.
- **One substantive community contribution:** answer a question, review a related tool, or share a reusable test. Posting frequency never overrides community rules or account eligibility.
- **One maintainer update:** fixes, new contributors, reproducible issues and the next evaluation. Ask for a specific contribution; credit real work.

Keep two or three hours available after a launch post to answer questions yourself. Do not automate replies, use engagement pods, purchase stars or trade votes. Recheck [platform guidance and subreddit restrictions](RESEARCH.md) before posting.

## A campaign built around evidence

| Story | Visual | Useful next action |
| --- | --- | --- |
| “The agent behind mlxtop” | Live billing repair GIF / MP4 | Run the fixture |
| “What did the frontier actually see?” | Privacy record → exact outbound text | Inspect and verify the audit |
| “Our own launch demo found a leak” | Failing canary test → fix → clean rerun | Reproduce it or add an attack case |
| “Keep the implementation local” | A future genuine protected-source edit | Try it on synthetic protected code; do not publish before the run exists |
| “No frontier for this repository” | Real top-clearance task and trap-endpoint tests | Test a local-only workflow with an approved endpoint |
| “Does privacy make coding more expensive?” | Paired result table, including failures | Reproduce the benchmark; propose a quality-preserving optimization |

The security story should accumulate evidence, not merely repeat “privacy.” Local-runtime projects, Rust/terminal maintainers, security practitioners and technical video creators are relevant potential collaborators. Approach a few whose existing work fits, with one runnable artifact and a specific evaluation idea. No outreach has been sent and no partner support is implied.

## Measurement and stop rules

Track a weekly table with UTC dates: total/new stars, unique repository visitors, unique cloners, linked post impressions/clicks when available, voluntary successful-trial reports, externally reproduced canary cases, return usage from opt-in reports, outside PRs and unresolved setup/security issues. Duet needs no usage telemetry for this campaign.

GitHub's traffic window is 14 days: save aggregate [traffic insights](https://docs.github.com/en/repositories/viewing-activity-and-data-for-your-repository/viewing-traffic-to-a-repository) weekly. Do not pretend it attributes every star to a post. Use named release/documentation links and the owner's platform analytics where available. Count installations separately from stars; raw clones include automation.

Copy this tracker weekly; leave unknown values blank rather than inferring them from stars:

| Week ending (UTC) | New / total stars | Unique visitors / cloners | Post impressions / clicks | Reported successful trials | Independent reproductions | Outside contributors | Next decision |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Day 0 baseline | | | | | | | |
| Week 1 | | | | | | | |
| Week 2 | | | | | | | |

Operational experiment thresholds below are proposed decision rules, not external benchmarks:

- After 1,000 qualified visitors, if fewer than 3% star, inspect the first screen and audience fit. After 20 independent trials, if fewer than 70% finish, prioritize setup and product fixes before more reach.
- If reach is high but visits are low, clarify the product and the one next action. If visits are high but trials are low, reduce install friction. If trials work but users do not return, find a recurring problem worth solving.
- Any confirmed disclosure path takes priority over promotional scheduling. Record its scope, fix and regression; never delete a failed result to improve the graph.
- Do not spend on distribution until the first-use funnel works. At that point, prefer a small, disclosed technical evaluation or tutorial experiment to broad untargeted ads. Set a fixed spend cap and judge cost per successful trial; no spend is authorized or assumed here.

## Time and ownership

Planning assumption: one maintainer can reserve approximately 6–8 hours/week for documentation, recording, community participation and measurement, in addition to engineering. If that capacity is unavailable, publish one strong demonstration every two weeks. Independent audits, signed releases and support require their own engineering capacity and budgets; campaign dates move with readiness.

Use [LinkedIn drafts](LINKEDIN.md), the [Reddit author brief](REDDIT.md), [research sources](RESEARCH.md) and the [demo packet](DEMO.md). All content is local draft material. Nothing has been posted, messaged, pushed or scheduled.
