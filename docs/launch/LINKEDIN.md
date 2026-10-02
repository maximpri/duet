# LinkedIn launch drafts

Drafts for the author's review and personal use; nothing has been posted. Publish against a reviewed public revision so every evidence link works. The primary post continues the actual mlxtop conversation without repeating private analytics as verified facts.

## 1. Launch: the coding agent behind mlxtop

**Attachment:** [duet-boundary.mp4](../assets/launch/duet-boundary.mp4). Cover: [fresh privacy panel](../assets/launch/duet-fresh-privacy.png).

The coding agent behind mlxtop has a question for your security team:

What did the model actually see?

I built Duet for engineering work that involves customer records, credentials and proprietary code.

Two models. Different access.

A frontier model handles coding. A local model handles sensitive content. Duet checks the information crossing between them, sandboxes tools and records outbound requests.

The clip is a real terminal run on fictional billing data:

→ Fix the bug and pass the tests.
→ Open the privacy decision.
→ Inspect the prepared outbound text.
→ Verify the audit.

Four tests passed. A separate checker found zero matches for 13 planted values across five recorded frontier requests. The recording, original requests and checker are in the repository. Local inference in this demo ran on my configured LAN server.

If a repository permits no frontier at all, top clearance uses only the approved local endpoint.

Duet is a development preview. The goal is frontier-quality work with less disclosure and lower cost. Today's paired privacy benchmark retains similar task results but costs more; I publish that tradeoff too.

Try the fictional-data task and inspect the boundary:
https://github.com/maximpri/duet

For teams evaluating coding agents: which evidence is hardest to obtain today—data flow, tool permissions or audit history?

## 2. Follow-up: the failure that improved the boundary

**Attachment:** [duet-audit.mp4](../assets/launch/duet-audit.mp4). This older recording is labelled separately in the evidence packet.

Our coding-agent demo passed every coding test.

Its privacy check failed.

While dogfooding Duet, we found a diagnostic preview that exposed fictional customer values. An email ending in .invalid made a CSV row look like an error message. Removing the name still left the ID, email and amount.

The next run found a second path: the local model copied short amounts into its summary.

We retained both failed audits, added regression tests, fixed both paths and reran the task. The complete-value checker then found zero matches. The limits of that check are documented alongside the records.

One lesson matters for anyone buying or building coding agents:

An intact audit log can faithfully record a disclosure.

Verify the chain. Inspect the content. Test the boundary independently.

Here are the failures, fixes and real terminal recordings:
https://github.com/maximpri/duet/blob/main/docs/launch/DEMO.md

I'm interested in synthetic cases that challenge this design, especially inference from summaries and repeated extraction attempts. Live vulnerabilities should go through the verified private reporting route once it is established.

## 3. Evaluation: a concrete packet for security teams

**Attachment:** [fresh audit screenshot](../assets/launch/duet-fresh-audit.png).

Before a coding agent touches a sensitive repository, I want answers to five questions:

1. Which model can see which data?
2. Can tools bypass that decision?
3. Can repository instructions weaken policy?
4. Can we inspect and verify the outbound record?
5. What happens when cloud disclosure is prohibited?

Duet's evaluation guide maps those questions to code, tests and actual dogfood evidence. It includes a synthetic pilot and identifies the operational work still needed: managed policy and identity, approved endpoints, protected audit custody, release provenance and independent assessment.

The design pairs frontier coding with local handling of sensitive content. Top clearance removes the frontier when that is the required policy.

This is an independent project in development, not a claim of bank certification or government approval.

The evaluation packet:
https://github.com/maximpri/duet/blob/main/docs/INSTITUTIONAL_EVALUATION.md

If you own developer tooling or security review, try the synthetic task and tell me where the evidence falls short.

## Publishing notes

- Use the native MP4 and a plain-text description. The GitHub GIF is an alternative, not evidence of real-time duration.
- Suggested media description: “Actual Duet terminal with fictional billing data. The agent fixes a status check; the operator inspects filtered context and verifies the audit. Pauses are cut. The linked packet contains the original request log and test results.”
- Put the repository link in the body. We found no primary evidence requiring a first-comment link or promising a reach multiplier.
- Use no more than a few directly relevant hashtags if desired; their effect is an experiment, not a guarantee. Do not gate access behind comments or script responses.
- Answer the existing public interest in Duet's architecture with substantive follow-up content. Contacting or replying to anyone is a separate owner action; no outreach has been sent.
- The earlier launch brief contains private impression/star figures and a former banking title. These drafts do not depend on them. Verify any personal credential or private metric before adding it.
- Recheck community/product readiness and replace future `main` links if publishing a different reviewed revision.
