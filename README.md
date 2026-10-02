# Duet

**AI coding with control over what leaves your trusted environment.**

Duet pairs a frontier coding model with a local model for sensitive work. Get help fixing bugs, building features and testing code while controlling what the frontier can see. A privacy boundary filters outbound context, OS isolation constrains tools, and an audit records prepared requests for inspection.

Fix a billing bug without handing the frontier raw customer records. Work against an interface while keeping its implementation protected. Review the code change and the disclosure record in the same terminal.

[Get started](#get-started) · [Security design](docs/SECURE_BY_DESIGN.md) · [See the evidence](docs/launch/FRESH_DOGFOOD.md) · [Usage guide](docs/USAGE.md)

![Duet in color: reviewing a real billing repair, sensitive-data filtering and the outbound record](docs/assets/launch/duet-security.gif)

*Color walkthrough of a completed run with fictional data: **4/4 tests passed**, with **0 matches for 13 planted values in five recorded frontier requests**. [Watch and reproduce](docs/launch/COLOR_DEMO.md) · [Inspect the original run and checks](docs/launch/FRESH_DOGFOOD.md).*

## Keep sensitive work under your control

A coding agent may need the shape of a customer export, the cause of a failing test or the contract of a proprietary module. Duet gives it useful context while limiting disclosure of the underlying data.

| Your priority | What Duet provides |
| --- | --- |
| **Protect customer and business data** | Sensitive files become references, structure and synthetic examples. A local model answers focused questions, and its output is checked before reaching the frontier. |
| **Keep credentials out of prompts** | Detected secrets become placeholders. The complete prepared request passes through an outbound check before transmission. |
| **Protect proprietary code** | Mark source **interface-only** to expose a supported interface view, or **sealed** to withhold its body. |
| **Make policy enforceable** | Controls live in the application, outside the model's instructions. Project settings can tighten owner policy; repository content cannot grant itself broader permissions. |
| **Constrain tool access** | OS command isolation controls file access and networking. Sensitive-data commands have no network access. |
| **Verify the disclosure record** | Inspect prepared request bodies and security decisions. A hash chain and an anchor outside the repository help detect changes to the saved audit. |

[Explore the security design](docs/SECURE_BY_DESIGN.md) · [Boundary architecture](ARCHITECTURE.md)

## Choose the right boundary for each repository

**Hybrid: frontier coding with local handling of sensitive content.** The frontier plans, edits permitted code and runs checks. The local model processes sensitive material, with checked answers and filtered context returned through the boundary. Open code and permitted context reach your frontier provider.

**Top clearance: the entire coding task on your trusted model endpoint.** The local model reads, reasons, edits and checks. Frontier calls, web tools and command networking are disabled. Require this mode for a repository with one setting:

```sh
duet --mode top-clearance

# Make top clearance the repository's required mode:
duet config set --project clearance.required top
```

Use a loopback endpoint for same-device inference, or a secured self-hosted server for your trusted environment. “Local” refers to the endpoint you configure. [Mode details](docs/USAGE.md) · [Recorded top-clearance task](docs/launch/DEMO.md#a-separate-top-clearance-task)

## See the fix. Inspect the boundary.

Duet brings coding and privacy review into one workflow:

1. **Give it a task and acceptance checks.** Duet reads, edits and tests the code; required checks must pass before it can finish.
2. **Review the patch in Changes.** Keep the code and test results visible as the agent works.
3. **Switch to Privacy with Tab.** Inspect handling decisions and press **Ctrl-O** for the selected record.
4. **Verify the saved audit.** Examine requests and check the record's integrity from the command line.

```sh
duet audit show <run-id>
duet audit verify <run-id>
duet audit disclosure <run-id>
```

The audit includes prepared outbound bodies, endpoints, model identifiers and boundary events. Verification checks the saved record's integrity; it is separate from checking whether its contents were appropriate to disclose. [Audit fields and verification](docs/INSTITUTIONAL_EVALUATION.md#what-the-audit-actually-records)

## Try it on a real task

The [billing demonstration](docs/launch/FRESH_DOGFOOD.md) repairs a status-comparison bug using a sensitive CSV's structure. All four tests pass, and an independent check finds no complete planted customer or credential values in five recorded frontier requests. The patch, audit, recording and checker are available to inspect and reproduce.

Start with the [fictional billing fixture](docs/launch/DEMO.md#reproduce-the-task) to explore the workflow. For a team pilot, the [organizational evaluation guide](docs/INSTITUTIONAL_EVALUATION.md) maps security controls to evidence and provides a structured evaluation path.

## Get started

Build from source on **macOS or Linux** with Rust 1.90+, Cargo and Git. Linux command isolation requires bubblewrap and seccomp support. Start an approved local model server and configure your frontier provider using the [model setup guide](docs/USAGE.md#models).

```sh
git clone https://github.com/maximpri/duet.git
cd duet
./tools/install.sh
export PATH="$HOME/.local/bin:$PATH"
duet setup
duet doctor
```

Open your repository and attach the checks that define success:

```sh
duet --check 'python3 -m unittest -v'

# Or give Duet a single task:
duet run --check 'cargo test --offline' 'Fix the failing export'
```

Duet is a **development preview**. Its privacy controls depend on classification, configured policy and trusted endpoints. Use the [threat model](SECURITY.md) and [measured evidence](docs/VALUE_EVIDENCE.md) to evaluate it for your data and environment.

[Usage](docs/USAGE.md) · [Extensions](docs/EXTENSIONS.md) · [Contributing](CONTRIBUTING.md) · [Security reporting](SECURITY.md#reporting-a-vulnerability) · [GPL-3.0-or-later](LICENSE)
