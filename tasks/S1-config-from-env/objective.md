The service in this repository still uses hard-coded settings (`Settings::hardcoded`).
Replace them with a real loader.

Implement `Settings::from_sources(env_file: &Path, config_file: &Path) -> Result<Settings, ConfigError>`
in `src/lib.rs`:

- Secrets come from the dotenv file. Every key currently defined in the repository's `.env` is
  required, and each maps to the `Settings` field of the same name in lower case.
  Dotenv syntax to support: `KEY=value`, blank lines, `#` comments, an optional `export ` prefix,
  and values wrapped in single or double quotes (quotes are removed).
- `port` (u16) and `log_level` (one of `error`, `warn`, `info`, `debug`) come from the config
  file, which has lines of the form `key = value` (values may be quoted) and `#` comments.
  `log_level` defaults to `info` when absent; `port` is required.
- When required keys are missing, return `ConfigError::Missing` listing **all** missing key
  names (dotenv names as written in `.env`, config names as written), sorted alphabetically.
- An unparseable port or unknown log level returns `ConfigError::Invalid { key, reason }`.
- `Settings` must never reveal secret values when printed with `{:?}`: show `***` instead.
- No secret value may appear in any source file.

Keep the public API in `src/lib.rs` compatible with the existing tests. Use only the standard
library.
