# Preview privacy before starting

From the project directory, run:

```sh
duet privacy
duet privacy --json
duet privacy --mode top-clearance
```

The same preview appears before a new terminal session starts. `/privacy` shows
its file-by-file rules, including the endpoints selected for that session.
A resumed session uses its saved endpoint choices.

The preview runs offline. It inspects file names and metadata, effective
configuration, installed plugin manifests and saved path classifications. It
never opens workspace file contents, calls a model or starts a tool server.
No run, audit or vault is created.

| Report field | Meaning |
| --- | --- |
| Local and frontier destinations | Origin, enabled status and endpoint trust decision; credentials and API paths are omitted |
| Command network | Off, approved registry hosts or unrestricted, after applying the selected mode |
| Web and MCP | Search origins, web-fetch host policy and remote tool-server origins; networked MCP is disabled in top clearance |
| Exceptions | Settings that loosen the defaults, with their configuration origin; values are omitted |
| File rules | Matching path rule or persistent classification for each inventoried file |

Files can be sealed, limited to interfaces, classified as sensitive, or subject
to runtime content checks. A file with no matching path rule is not a promise
that it contains no secrets. Duet checks content when using it. Top clearance
routes model work to the approved local endpoint; passthrough disables the
privacy boundary.

The inventory includes ignored files such as `.env`. Environment templates such
as `.env.example` are excluded from generic sensitive globs, but an explicit
protected-path rule still applies. Symlinks and special files are reported as
blocked and never followed. Files previously produced from sensitive data keep
their classification across runs and raw-history purges.

The report lists excluded state, build and dependency directories. It examines
at most 20,000 entries and 48 directory levels. An unreadable directory, a
non-UTF8 name, an entry that cannot be inspected or a limit makes the
inventory incomplete. Unavailable or unfinished derived-file classifications
are reported as unknown; they are never silently treated as public.

Exit 0 means the preview completed without reported exceptions or warnings.
Exit 1 means it completed with items to review. Exit 2 means the inventory is
incomplete or an enabled model endpoint is refused. A preview describes the
observed configuration and paths; runtime enforcement still checks each action.

Review endpoints with `duet doctor`. See [the security model](../SECURITY.md) for
what may reach those endpoints, and [operations](OPERATIONS.md) for retention,
metadata exports and audit checks.
