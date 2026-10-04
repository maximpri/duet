# Duet

**Frontier coding. Local privacy controls.**

Duet is an AI coding assistant for your terminal. A frontier model plans, writes and fixes code. Your local model handles sensitive files, and Duet checks the context sent to the cloud.

[Install](#install) · [Get started](#get-started) · [Results](#results) · [Documentation](docs/README.md) · [Downloads](https://github.com/maximpri/duet/releases/latest)

- **Choose what the frontier sees.** Share ordinary code, expose a private module's interface, or keep its implementation sealed.
- **Inspect the work and the disclosure.** Review patches in **Changes**, outbound context in **Privacy**, and saved audit records.
- **Use your own model for the whole task.** Local-only mode disables frontier calls, web tools and command networking.

![Actual Duet session fixing a billing bug: the code change appears beside four passing tests.](docs/assets/billing-demo/04-completed.jpg)

*A real Duet session with fictional customer data. The billing fix passes all four tests.* [Try the example](docs/launch/DEMO.md#reproduce-the-task) · [Screenshots and outbound-request checks](docs/evidence/billing-demo-2026-10-04/README.md)

## Install

**macOS and Linux · Apple Silicon/ARM64 and Intel/x86-64**

```sh
curl --proto '=https' --proto-redir '=https' -fsSL https://raw.githubusercontent.com/maximpri/duet/main/install.sh | bash -s -- --allow-unsigned
```

The installer selects the latest release, checks its files and installs to `~/.local/bin`. It retains the matching GPL source and licenses. No Rust compiler or administrator access is needed.

Preview releases are **unsigned**. Checksums detect changed files; they do not authenticate the publisher. This command trusts GitHub over HTTPS. Signing is optional.

[macOS disk images and Linux archives](https://github.com/maximpri/duet/releases/latest) · [Inspect the installer, verify signatures or build from source](docs/INSTALLATION.md)

Linux requires glibc, bubblewrap, user namespaces and seccomp support for command isolation. macOS disk images are not Apple-signed or notarized.

## Get started

For hybrid mode, start a compatible local model server and set your frontier provider's API-key environment variable. [Model setup](docs/USAGE.md#models) covers supported providers and local endpoints. `duet setup` discovers available models; it does not download them.

From your project directory:

```sh
export PATH="$HOME/.local/bin:$PATH"
duet setup
duet doctor
duet privacy
duet
```

Describe what you want to build or fix. To require passing tests before Duet finishes, give it your project's test command:

```sh
duet --check 'python3 -m unittest -v' "Fix the failing billing tests"
```

To use only your approved local model:

```sh
duet --mode top-clearance
```

## Results

**Near-frontier automated-test scores, with zero observed copies of planted private values.**

We ran nine coding tasks with three paired seeds: **54 scored outcomes**, using the same frontier model in both modes. Hidden tests checked the resulting code.

| Measurement | Duet hybrid | Frontier with privacy boundary disabled |
| --- | ---: | ---: |
| Mean automated-test score | **89.26%** | **96.54%** |
| Literal planted-value matches in recorded requests | **0** | **35,809** |
| Recorded frontier requests checked | 1,124 | 1,328 |

Duet scored within **7.28 percentage points** of the frontier baseline. All selected outcomes count, including an externally stopped hybrid run scored zero. These are coding-test scores and literal-value checks: they do not establish general quality parity or rule out disclosure through summaries.

[Results infographic and task breakdown](docs/DUET_VISUAL_GUIDE.md#results-by-task) · [Method and complete evidence](docs/evidence/benchmark-54-2026-10-04/README.md)

## How privacy works

You choose which files and model endpoints to trust. Sensitive files go through local handling; the frontier works with permitted code, references, structure views and checked answers. The application enforces outbound policy, and the operating system isolates commands.

“Local” means your workstation or an approved self-hosted endpoint. Hybrid mode sends permitted code and checked context to the frontier. Summaries can still reveal private facts, so inspect your policy with `duet privacy` before starting.

[Architecture and modes](docs/DUET_VISUAL_GUIDE.md) · [Security design](docs/SECURE_BY_DESIGN.md) · [Audit and retention](docs/OPERATIONS.md)

<details>
<summary>Watch the privacy controls in Duet's terminal</summary>

![Color walkthrough made from actual native Terminal screenshots of Duet's privacy, outbound-context and code-change views.](docs/assets/launch/duet-security.gif)

This earlier walkthrough uses unchanged native screenshots, held for five seconds each. Playback length does not represent task duration. [Capture details and verification](docs/launch/DEMO.md#native-screenshots).

</details>

## Documentation and feedback

Duet is a development preview. Start with the included example and share what worked or got in your way.

- [Usage and configuration](docs/USAGE.md) · [Extensions](docs/EXTENSIONS.md) · [All documentation](docs/README.md)
- [Report a bug or suggest an improvement](https://github.com/maximpri/duet/issues)
- [Report a vulnerability privately](https://github.com/maximpri/duet/security/advisories/new) · [Security policy](SECURITY.md)
- [Contributing](CONTRIBUTING.md): outside code contributions are closed until the CLA is published. Bug reports and feedback are welcome.

## License

[GPL-3.0-or-later](LICENSE). Release archives include corresponding source and third-party notices. See [licensing and distribution](LICENSES.md).
