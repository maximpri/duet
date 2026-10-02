# Instructions, skills and plugins

Reuse repository conventions, add a repeatable workflow, or connect a tool server.
All three use Duet's existing privacy, permission and budget controls.

| Need | Use | When it is loaded |
|---|---|---|
| Build commands, layout and coding conventions | `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, `DUET.md` | At session start, with relevant directory rules on file access |
| A workflow such as code review | A directory containing `SKILL.md` | Short description first; instructions and resources when needed |
| Distribute skills, prompt commands and optional MCP servers | A native `duet-plugin.toml` package | After the owner installs and enables it |

These features are part of the development build. The compatibility and limits
below describe Duet's implementation, rather than every feature of another agent.

## Try the review example

From this checkout, after [installing Duet](../README.md#getting-started):

```sh
duet plugins inspect examples/plugins/quality-kit
duet plugins install examples/plugins/quality-kit
duet skills list
duet skills show quality-kit:code-review
duet
```

In the new workspace session, enter either:

```text
/skill quality-kit:code-review Review the uncommitted changes
/command quality-kit:review Review the uncommitted changes
```

The [example package](../examples/plugins/quality-kit/duet-plugin.toml) contains
one review skill and one prompt command. It has no scripts, credentials or MCP
servers. Reading or installing it makes no model request; using the workflow in a
session uses that session's configured models and budgets.

## Repository instructions

Existing `AGENTS.md` files can describe build commands, tests and conventions;
this follows the purpose of the [AGENTS.md format](https://agents.md/).
Duet also reads `CLAUDE.md`, `GEMINI.md` and its own `DUET.md` as plain instruction
text. It does not interpret those products' settings formats.

At the repository root the order is:

1. `AGENTS.override.md`, when present; otherwise `AGENTS.md`.
2. `CLAUDE.md`.
3. `GEMINI.md`.
4. `.github/copilot-instructions.md`.
5. `DUET.md`.

Later files provide the more specific conventions within that directory. An
override replaces only `AGENTS.md`. An unreadable or withheld override does not
silently fall back to the replaced file. Within Duet's host rules, the precedence
is **the current user request, then owner instructions, then repository guidance**.
More specific repository directories take precedence over their ancestors, with
`DUET.md` last within a directory. File text cannot change Duet's security rules.

Your standing instructions live next to your owner configuration, normally
`~/.config/duet/`: `AGENTS.md` or its override, `CLAUDE.md`, `GEMINI.md`, then
`DUET.md`. These are loaded before the repository files. `DUET_CONFIG_HOME`
changes that directory; Duet does not search unrelated home directories for
standing instruction files.

### Directory scope and resume

Place a supported instruction file inside a directory to apply it to that
directory and its descendants. For `src/parser/token.rs`, Duet checks `src/` then
`src/parser/` when its `read_file`, `edit_file`, `write_file`, `edit_protected` or
`rename` tool names that file. Deeper rules take precedence. Copilot instructions
are root-only.

When a file operation reveals new rules, Duet returns the rules first and asks the
model to repeat the operation after reading them. Unchanged directory rules are
not repeatedly inserted; edits to those files refresh the relevant scope.
Duet instructs the model to read a file before changing it through a shell
command, so applicable guidance can be discovered. Shell paths and the other
files affected by a language-server rename are not automatically enumerated by
this guard. Keep indispensable project-wide guidance at the root.

Root instructions enter the opening message once and are replayed on resume.
Start a new session to pick up changed root instructions. Descendant rules are
checked again after resume or context is discarded. File references such as
`@other-file.md` remain ordinary text; they are not expanded automatically.

Each instruction file contributes at most 16 KiB. Combined root instructions are
capped at 64 KiB, and each scoped block at 32 KiB; omitted text is marked. Files
over 2 MiB, symlinks and nonregular files are refused. Project instructions pass
through repository path visibility and content filtering; owner instructions are
sanitized like operator messages. In hybrid mode, a sensitive value in an
instruction file does not gain permission to cross the outbound gate.

## Portable skills

A skill uses the standard directory layout and YAML-frontmatter-plus-Markdown
`SKILL.md` format. Resources can live beside it and load when needed. See the
[Agent Skills specification](https://agentskills.io/specification).

For example, create `.agents/skills/code-review/SKILL.md`:

```markdown
---
name: code-review
description: Review a diff for concrete regressions and verify findings with focused checks.
---

Review the requested changes and their callers. Report reproducible defects
with file and line references, then the checks performed and any coverage gaps.
```

The name must match the directory: 1–64 lowercase ASCII letters, digits or single
hyphens, without a leading or trailing hyphen. A description is required and can
contain up to 1,024 characters. Use plain or quoted strings; descriptions can also
use YAML `|` or `>` blocks. Duet parses these fields, not arbitrary YAML objects,
tags or aliases. Extra metadata is retained as content and grants no capabilities.

### Automatic discovery

Duet checks these roots in order. Each contains `<name>/SKILL.md` directories;
the first matching skill ID wins, and duplicates appear as diagnostics.

| Order | Root | Origin |
|---|---|---|
| 1 | `~/.config/duet/skills/`, or the skills directory beside the owner config | Owner |
| 2 | `~/.agents/skills/` | Owner |
| 3 | `~/.claude/skills/` | Owner |
| 4 | `~/.config/opencode/skills/` | Owner |
| 5 | `.duet/skills/` in the workspace | Project |
| 6 | `.agents/skills/` in the workspace | Project |
| 7 | `.claude/skills/` in the workspace | Project |
| 8 | `.opencode/skills/` in the workspace | Project |
| 9 | Skill roots declared by enabled installed plugins | Plugin |

When `DUET_CONFIG_HOME` is set, root 1 becomes `$DUET_CONFIG_HOME/skills/` and
the other owner-home roots (2–4) are skipped. Project roots still apply. Native
plugin skills have qualified IDs such as `quality-kit:code-review`, so they do
not shadow an unqualified `code-review`.

Use `duet skills list` or `/skills` to see what was found. Read a skill locally
with `duet skills show code-review`, or a bundled resource with
`duet skills show code-review references/checklist.md`. These terminal inspection
commands read local content for the operator; they are not previews of the
filtered content that a frontier model will receive.

### Activation and privacy

The model receives a bounded catalog of names and short descriptions. It can
request another catalog page with `list_skills`, then load instructions with
`load_skill`. Full instructions and referenced resources are kept out of the
initial catalog and tool schemas. Use `/skill code-review <task>` to select a
skill explicitly. Reopen the session after adding or changing skills so its
catalog is refreshed.

| Optional frontmatter | Duet behavior |
|---|---|
| `disable-model-invocation: true` | Hidden from model discovery and unavailable to `load_skill` until the operator invokes it with `/skill` |
| `user-invocable: false` | Rejects explicit `/skill` invocation; automatic model use remains available unless separately disabled |
| `allowed-tools`, `model`, `context`, hooks or other tool-specific fields | Retained as content; no tool grants, model switch, process launch or hook execution |

By default both automatic and explicit use are available. Authorization for an
explicit-only skill lasts for the running session and its delegates. On resume,
previously loaded text remains in the transcript, but loading that skill again
requires another explicit `/skill` invocation.

Project metadata and documents use the same path checks and filtered views as
repository files. Skills on paths hidden by policy are omitted from model
discovery and cannot be loaded by name. Visible sealed or sensitive skill files
receive the boundary's notices, handles or filtered views; their raw bodies are
withheld. Owner and installed-plugin content is sanitized like operator messages.
All frontier requests still pass the outbound gate. A loaded
document records an `instructions` audit event with its skill ID, byte count and
SHA-256 digest.

Discovery does not execute code or contact a server. Documents are bounded at
128 KiB, loads reject symlinks and path traversal, and each load rechecks the
discovered `SKILL.md` digest. Referenced text resources load relative to the skill
directory. A script named in a skill is not run by `load_skill`; any later command
uses Duet's existing tools, sandbox and approvals.

## Native plugin packages

A plugin is an owner-installed local directory with a versioned manifest:

```text
quality-kit/
├── duet-plugin.toml
├── skills/
│   └── code-review/
│       └── SKILL.md
└── commands/
    └── review.md
```

```toml
schema_version = 1
name = "quality-kit"
version = "0.1.0"
description = "Review code changes with reproducible findings and focused checks."
skills = ["skills"]
commands = "commands"
mcp = []
```

Names are lowercase slugs. Paths are relative and cannot traverse out of the
package. `skills` defaults to `["skills"]`, `commands` to `"commands"`, and `mcp`
to an empty list. Unknown manifest fields are rejected. Native packages currently
contribute skills, Markdown prompt commands and MCP servers.

### Install, update and remove

```sh
duet plugins inspect ./quality-kit   # inspect manifest, capabilities and content digest
duet plugins install ./quality-kit   # private snapshot; enabled for the next session
duet plugins list
duet plugins disable quality-kit    # disable for future sessions
duet plugins enable quality-kit     # verify pinned content, then enable
duet plugins remove quality-kit     # unregister; retain the snapshot for recovery
```

Installation accepts a local directory, with no install scripts, package-manager
calls, marketplace fetch or dependency installation. It rejects symlinks, special
files and traversal; limits include 512 files, 8 MiB per file and 32 MiB per
package. The private store is beside the owner configuration under `plugins/`.
Duet resolves the owner-selected configuration directory once, so macOS `/var`
aliases and linked configuration directories work. Symlinks within the plugin
store and source package are refused.
Package content, including its manifest, is pinned by SHA-256 and checked when
the installed package is loaded. This detects changed content; it does not prove
the publisher's identity or code safety.

To update, inspect and install the revised source directory again. Duet records a
new content snapshot rather than modifying the old one. Restart the session to
refresh its skill catalog and tool servers. Disabling or removing a package does
not erase instructions already loaded or terminate a server in an active session;
close that session to end its tool processes.

`/command quality-kit:review <arguments>` reads `commands/review.md` as task
guidance and appends the operator's arguments. It does not perform shell
substitution, expand `$ARGUMENTS`, or execute backticks. The model can choose
normal tools to carry out the request. Packaged commands do not implicitly
authorize a skill marked `disable-model-invocation`.

### Optional MCP tool servers

An installed package can declare one or more `[[mcp]]` entries. Inspecting and
installing the package does not start them; an enabled package contributes them
when a run or session starts. Every contributed setting is checked against
organization policy, then passed to Duet's existing MCP client.

For example, a package with an already reviewed `server.py` could declare:

```toml
[[mcp]]
name = "review"
command = "python3"
args = ["${DUET_PLUGIN_ROOT}/server.py"]
trust = "sensitive"
network = false
approve = "always"
timeout_seconds = 60
env = []
```

This is a manifest example, not a server included in `quality-kit`. Using an
interpreter needs no executable bit on the script. `${DUET_PLUGIN_ROOT}` expands
only in `command` and `args`, to the installed snapshot. Stdio servers run in the
command sandbox with the workspace as their working directory; extra environment
variables must be named in `env`.

An HTTP server uses `url` instead of `command`. URLs require HTTPS, except HTTP
on loopback. Supply credential variable names through
`headers_env = ["Authorization=REVIEW_AUTH"]`; the variable holds the complete
header value. Never put credential values in a manifest. Choose exactly one of
`command` or `url`.

Plugin server defaults are `trust = "sensitive"`, `network = false`,
`approve = "always"` and a 60-second timeout. Approval behavior follows the
session's oversight mode: `always` asks for each call under `risky`, `all` asks
for every tool call, and `off` does not ask. Result filtering and outbound
argument checks still apply. In top clearance, HTTP servers and stdio servers
declared with network access are not started. See the
[MCP controls](USAGE.md#coding-with-duet-the-workspace) and
[security model](../SECURITY.md) for transport, sandbox and data-handling details.

### Workspace controls

Both extension features are enabled by default. A repository can disable them:

```sh
duet config set --project extensions.skills_enabled false
duet config set --project extensions.plugins_enabled false
```

Disabling skills removes skill discovery, including plugin skills. Disabling
plugins removes package contributions; ordinary project and owner skills still
work. Repository configuration cannot install or enable owner packages, loosen
these controls, or grant permissions through an instruction file.

## Compatibility at a glance

| Existing format or feature | Duet support |
|---|---|
| `AGENTS.md` / `AGENTS.override.md` | Root and directory scope, using Duet's ordering and limits above |
| `CLAUDE.md`, `GEMINI.md`, `DUET.md` | Plain instruction text at root and directory scope; no automatic `@file` includes |
| `.github/copilot-instructions.md` | Repository root only; `.github/instructions/*.instructions.md` globs are not interpreted |
| Standard `SKILL.md` with `name`, `description` and Markdown | Supported, with lazy text-resource loading and the parser limits above |
| Claude-style invocation flags | `disable-model-invocation` and `user-invocable` are enforced as documented above |
| Claude Code plugin manifests and marketplaces | Not imported; adapt portable skills and commands into `duet-plugin.toml` |
| Foreign Claude/DeepSeek plugin ABIs, hooks, install scripts or runtime APIs | Not executed or emulated; choosing a model provider does not add a plugin runtime |
| Native Duet skills and prompt commands | Supported through local package installation |
| MCP stdio / streamable HTTP | Supported through owner configuration or a native package, with Duet's security controls |
| Native plugin dependency resolution, automatic updates, signing or marketplace | Not implemented |

Claude Code defines a separate plugin manifest and runtime, including hooks and
other components; those features require explicit adaptation. See its
[official plugin manifest reference](https://code.claude.com/docs/en/plugins-reference).
Duet's portable-skill support does not imply compatibility with that plugin runtime.

## Verification

The [local validation record](evidence/extensions-validation-2026-10-01.md)
includes test counts, plugin lifecycle checks and environment limitations.
Quality and cost claims for this extension-enabled prompt still require task benchmarks.
