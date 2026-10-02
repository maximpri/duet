# Launch research and decisions

Research checked 2026-10-01, America/Toronto (some captures after midnight UTC). This is a launch hypothesis, not a prediction of virality. Platform rules can change; recheck the linked primary sources on posting day.

## Start with the success we actually have

Maxim reports **50,000 LinkedIn impressions and 90 GitHub stars** for mlxtop. Treat those as owner-supplied launch figures, not independently measured attribution. The public [mlxtop post](https://www.linkedin.com/posts/maximpriezjev_a-top-for-local-llms-on-your-mac-ive-activity-7504045817205272576-7uJZ) showed 232 reactions and 15 comments when inspected. Public comments include interest in Duet's architecture and in trying Duet itself. Impressions are not publicly verifiable here.

The useful continuation is: **“mlxtop was the output. Here is the coding agent behind it—and the data boundary inside that agent.”** Link the [working mlxtop repository](https://github.com/maximpri/mlxtop) as an artifact. It demonstrates that useful software was built; it does not establish that Duet prevents all leaks or has equivalent quality on every task.

An earlier [July 29 Duet article](https://www.linkedin.com/pulse/duet-coding-agent-cut-model-costs-35-benchmarked-maxim-priezjev-6ycnc) reports 64% savings for a split implementation workflow. The current checkout's paired privacy evaluation uses a different design and reports a cost premium. Do not carry that earlier percentage into this release without recovering the exact implementation, artifacts and comparable evaluation. A clear explanation of the changed architecture is better than contradictory claims.

## What effective READMEs make easy

These are observations from the projects, not causal explanations of their popularity.

| Reference | Observed pattern | Duet decision |
| --- | --- | --- |
| [mlxtop](https://github.com/maximpri/mlxtop) | Familiar one-line analogy, screenshot immediately after the description, early “Try it” | Put the real TUI and practical outcome on the first screen |
| [Crush](https://github.com/charmbracelet/crush) | Distinctive identity, visible terminal demo, concise feature list, installation | Make the boundary visible; keep the feature catalogue in usage docs |
| [Aider](https://github.com/Aider-AI/aider) | Clear category, terminal screencast and quick-start material | Answer “what is it?” before explaining internals |
| [GitHub README guidance](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-readmes) | Purpose, usefulness, getting started and help belong in the README | Move detailed security evidence to a linked guide; use repository-relative links |

The first screen should make one argument: **capable coding help with an inspectable disclosure boundary**. A synthetic-data demo is the visual proof. Short navigation takes evaluators to the threat model and reproducible tests. Avoid certification-shaped badges, unsupported superiority claims, invented customer logos, and a wall of badges before the product.

## What LinkedIn's own guidance supports

LinkedIn's [March 2026 feed update](https://news.linkedin.com/2026/ImprovingTheFeed) says it is reducing repetitive, low-substance content and engagement bait and acting against automated comments and artificial engagement. Our inference: first-person engineering experience, a real artifact and a specific question fit better than generic superlatives or “comment DEMO” bait. There is no evidence here for an exact best time, a guaranteed link penalty, or a reach multiplier.

Use the former **Senior Managing Architect at TD Bank** role once to explain the perspective. It is an owner-supplied credential, not a bank's endorsement. Lead with a technical decision and show its consequence. Upload the included MP4 for the terminal clip: LinkedIn [supports MP4 uploads](https://www.linkedin.com/help/linkedin/answer/a548372). The GIF remains useful in GitHub. Write an accompanying text explanation for readers who cannot read a small terminal video.

Test two hooks on different substantive posts, without reposting identical copy:

- Founder context: the architecture question a former banking architect wants answered.
- Technical evidence: the failed canary test, the fix, and the replayable audit.

Evaluate repository visits, successful trials and useful responses alongside impressions. A security lead reproducing a failure is more useful than an empty compliment.

## Reddit: fit and rules first

[Reddit's spam policy](https://support.reddithelp.com/hc/en-us/articles/360043504051-Spam) prohibits repetitive mass engagement and directs participants to community-specific rules. Do not cross-post the same sales copy, coordinate votes, automate replies or hide authorship.

| Community | Rules observed | Appropriate action |
| --- | --- | --- |
| [r/LocalLLaMA](https://www.reddit.com/r/LocalLLaMA/) | LLM relevance, affiliation disclosure, limited self-promotion; primarily LLM-generated copy/code prohibited, with a narrow disclosed language-assistance exception | Strong subject fit for the local reader and leakage tests. Provide a factual brief; Maxim should author the actual post and confirm the project's eligibility. Do not paste the AI draft or use the exception as a workaround |
| [r/opensource](https://www.reddit.com/r/opensource/) | OSI license required; promotional flair for a project; no drive-by posting; AI-generated content treated as low effort | GPL license meets the license dimension, not all posting rules. Use the factual brief only, and independently author a compliant contribution |
| [r/rust](https://www.reddit.com/r/rust/) | Rust relevance and community moderation apply; AI-content moderation is actively discussed in [the moderators' policy thread](https://www.reddit.com/r/rust/comments/1qptoes/request_for_comments_moderating_aigenerated/) | Do not treat “written in Rust” as sufficient. Consider an original technical account of the gate, sandbox or regression if the rules permit it |

For other communities, check the actual rules before naming a posting date. A security-oriented audience can be reached through a maintainer-authored technical write-up and an invited discussion; it need not be a promotional subreddit launch.

## Secure by design is a product obligation

[CISA's description of the secure-by-design principles](https://www.cisa.gov/news-events/news/applying-secure-design-thinking-events-news) emphasizes responsibility for customer outcomes and transparency. Our practical application is to publish mechanisms, failures and fixes and make secure defaults verifiable. This is not CISA approval of Duet.

The [security evidence guide](../SECURE_BY_DESIGN.md) contains the banking/public-sector evaluation references and the unresolved deployment requirements. Keep regulation out of the headline: “GDPR compliant,” “bank approved,” “air-gapped” and “the most secure” require evidence this project does not have.

## Distribution beyond one post

[Show HN's guidelines](https://news.ycombinator.com/showhn.html) favor something people can actually try and ask creators to participate; they reject signup-only launches and vote solicitation. A clean install and the fictional billing fixture should be ready before that submission.

[GitHub traffic insights](https://docs.github.com/en/repositories/viewing-activity-and-data-for-your-repository/viewing-traffic-to-a-repository) expose a rolling 14-day window of visitors and full clones for users with push access. Record these at least weekly. They are aggregate indicators, not precise attribution of stars to a channel.

## Claim decisions for this launch

| Requested positioning | Publishable version | Evidence needed for a stronger version |
| --- | --- | --- |
| Most secure/private coding agent | Designed around an inspectable privacy boundary; explicit threat model | Independent comparative evaluation across defined threats and competitors |
| Frontier-level results | Near-parity results in the named paired development suite | Fresh, broader and independently reproduced quality results |
| Fraction of the cost | A stated target; current paired privacy runs cost more | Same tasks, model, seeds and quality gate; total cost and failed attempts included |
| Every bank/government must use it | For teams evaluating coding agents under strict data-handling requirements | Organization-specific assessment, operational controls and approved deployment |
| Proven secure | Tested controls and observed run evidence, with limits | No finite benchmark proves universal security |

The campaign uses the strongest current claim and gives people the means to challenge it.
