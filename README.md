# Duet

**Frontier coding for private work.**

Duet brings frontier models to confidential software work. It is designed for banks, government security teams and organizations building systems around data they cannot share with a model provider.

The frontier model plans the work, writes code and fixes failing tests. A local model handles sensitive files. Duet checks what passes between them, so the frontier can use a file's structure and checked answers while its raw contents stay inside your trusted environment.

Fix a financial reporting pipeline. Debug against confidential logs. Work with a proprietary algorithm through its interface. Keep control of the data behind the code.

![Duet running in macOS Terminal: inspecting a sensitive file, its outbound record and the code change](docs/assets/launch/duet-security.gif)

*Actual Terminal window captures from a completed task with fictional data. Four tests passed; an independent check found none of 13 planted private values in five recorded frontier requests. [Screenshots](docs/launch/COLOR_DEMO.md) · [Run and verification](docs/launch/FRESH_DOGFOOD.md)*

## Frontier results, private context

The two models have different jobs. The frontier does the coding. The local model reads sensitive content and answers questions about it. Duet checks those answers before they enter the frontier's context.

- **Raw sensitive files stay behind the boundary.** Files classified as sensitive are represented by references, schemas and synthetic examples. Detected credentials become placeholders.
- **You decide how much code to share.** Keep ordinary code available to the frontier, expose only the interface of a private module, or seal its contents.
- **The application enforces the rules.** Sandboxed commands and outbound checks apply regardless of what a model or repository instruction asks for.
- **Check the policy before you start.** `duet privacy` previews file rules, model destinations and policy exceptions offline.
- **You can inspect the result.** Review the patch in **Changes** and what was prepared for the frontier in **Privacy**. Saved audits can be checked for tampering.

In our recorded six-task comparison, with 18 runs per lane, Duet passed **98.3% of hidden tests**, versus **97.5%** with the same frontier model and the privacy boundary disabled. No planted private values appeared in the hybrid lane's captured outbound traffic. [Results and methodology](docs/VALUE_EVIDENCE.md)

Hybrid mode sends permitted code and checked answers to the frontier. Choose `duet --mode top-clearance` when source data and conclusions must stay with your own model: it disables frontier calls, web tools and command networking. Use a model on your machine or a trusted self-hosted endpoint.

[Security design](docs/SECURE_BY_DESIGN.md) · [Threat model](SECURITY.md) · [Measured results](docs/VALUE_EVIDENCE.md)

## Install on macOS or Linux

One-line source install. Requires **Rust 1.90+, Cargo, Git and a C/C++ toolchain** (Xcode Command Line Tools on macOS). Linux also needs **bubblewrap, user namespaces and seccomp support** for command isolation.

```sh
git clone https://github.com/maximpri/duet.git && ./duet/tools/install.sh
```

The installer builds Duet and puts it in `~/.local/bin`. Add that directory to your shell's `PATH`, then connect your frontier provider and local model:

```sh
export PATH="$HOME/.local/bin:$PATH"
duet setup
duet doctor
duet privacy
```

[Model setup](docs/USAGE.md#models) · [Verified GitHub release installation](docs/INSTALLATION.md#install-a-verified-binary)

Binary installation is ready for owner-published releases on macOS and Linux, on ARM64 and x86-64. It verifies the release against a signing identity you trust and retains the matching GPL source and notices.

## Use it

From your project directory:

```sh
duet
```

Describe the task. Attach tests when you want Duet to verify the fix before finishing:

```sh
duet --check 'python3 -m unittest -v'
```

Try the [fictional billing example](docs/launch/DEMO.md#reproduce-the-task), or read the [usage guide](docs/USAGE.md) for configuration, audit commands and local-only work.

Evaluating Duet for your organization? The [deployment evaluation guide](docs/INSTITUTIONAL_EVALUATION.md) maps controls to evidence and walks through a pilot with synthetic data. Duet is a development preview; the [publication review](docs/PUBLISH_READINESS.md) records verified checks and remaining work, and the [security documentation](SECURITY.md) defines its trust assumptions and protection scope.

[Privacy preview](docs/PRIVACY_PREFLIGHT.md) · [Audit and retention](docs/OPERATIONS.md) · [Independent review brief](docs/SECURITY_REVIEW_BRIEF.md)

[Contributing](CONTRIBUTING.md) · [Extensions](docs/EXTENSIONS.md) · [GPL-3.0-or-later](LICENSE) · [Third-party notices](LICENSES.md)
