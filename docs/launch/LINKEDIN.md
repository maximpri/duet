# LinkedIn drafts

Drafts for Maxim's review. Use the native MP4 clips from `docs/assets/launch/`; the GitHub GIF is also included. Publish only when the linked revision and evidence are public and the campaign's release prerequisites are met. No post or outreach has been sent.

The former TD Bank role is owner-supplied. These drafts describe an independent personal project and imply neither TD Bank endorsement nor an institutional deployment.

## 1. Founder launch — recommended first post

**Attachment:** `duet-privacy.mp4`.

My mlxtop post reached 50,000 impressions. The project reached 90 GitHub stars.

Here’s the coding agent behind it: Duet.

I’m a former Senior Managing Architect at TD Bank. The question I want a coding agent to answer is simple:

What, exactly, did you send outside my environment?

Duet is built around that question.

A frontier model plans and writes code. A model you control handles sensitive content. The host enforces the boundary: classification, sandboxed tools, checked outbound requests and an audit trail you can inspect.

In the clip, Duet fixes a billing bug using fictional customer data. You can follow the privacy decision, inspect the code change and verify the outbound audit.

That run passed all four task tests. A separate checker found zero matches for 13 complete planted values across four recorded frontier requests. The check's scope and original records are public with the demo.

There’s also a top-clearance mode: the configured local model does the work, with no frontier, web tools or command networking.

This is a development preview. The current paired privacy benchmark retained similar task results but cost more. Lower cost remains a goal. The security evidence includes a leak our own dogfood found, its fix and the rerun.

Code, demo and reproducible evidence:
https://github.com/maximpri/duet

If you evaluate coding agents for sensitive repositories, what evidence would you need before trying one?

## 2. Engineering follow-up — publish the failure and fix

**Attachment:** `duet-audit.mp4`, plus a direct link to the final published demo evidence.

Our coding-agent launch demo passed its tests.

Then we checked what it sent to the model.

A diagnostic preview had exposed fictional customer values. An email ending in `.invalid` was enough to make a CSV row look like an error line. The detector removed the name but left an arbitrary customer ID, email and amount.

The code fix worked. The privacy check failed.

We preserved the failed audit, reproduced the disclosure in a test that records the frontier's requests, and removed that preview path for non-log sensitive files. The next run caught another gap: the local summary quoted exact amounts. We added a structured-value check too, then reran the task against the same canaries.

That’s the standard I want for Duet: a security claim with an implementation, a test and evidence someone else can inspect.

A hash chain makes a record tamper-evident. It doesn’t make the recorded content safe. You have to inspect both.

The before/after evidence and limits are in the repo:
https://github.com/maximpri/duet/blob/main/docs/launch/DEMO.md

What other paths would you test—tool output, local summaries, plugins, or derived files?

## 3. Institutional evaluation — after release blockers are closed

**Attachment:** `duet-privacy.png` or a readable audit screenshot. Use a still when the relevant text is too small in video.

“Secure by design” should come with something an evaluator can inspect.

I’m building Duet with a specific split: a frontier model handles coding decisions; a model you control handles sensitive content; the host checks the boundary.

As a former Senior Managing Architect at TD Bank, I want the evaluation to start with concrete questions:

• Which content can reach a provider?
• Can a command or plugin bypass that decision?
• Can repository instructions weaken policy?
• What record exists of the outbound requests?
• What changes when the repository permits no frontier at all?

Duet’s evidence guide links those questions to the code, adversarial tests and actual dogfood runs. It also names what is still missing for an institutional deployment, including independent assessment and managed audit custody.

This is an independent project in development. It is not bank-certified, government-approved or a substitute for an organization’s own assessment.

If you are evaluating this problem, the synthetic pilot and threat model are here:
https://github.com/maximpri/duet

## Editorial notes

- Keep “secure by design” tied to mechanisms, not “unhackable,” “zero risk” or “most secure.”
- The first post uses the owner's mlxtop figures; do not call those Duet adoption metrics.
- Do not reuse the older 64%-savings article as evidence for this architecture. See the research note.
- The LAN endpoint limitation must stay in the linked demo and caption. It is not a same-device demonstration.
- Links above assume the work is merged into `main`; update them to the actual public release before posting.
- Add video alt/context text: “Actual Duet terminal run with fictional billing data. Privacy panel shows handling; acceptance tests pass. The linked audit records and canary check give the evidence.”
- Answer comments personally. Do not script engagement, request coordinated likes or gate the repo link behind a comment.
