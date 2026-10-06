// SPDX-License-Identifier: GPL-3.0-or-later
//! Credential stores under the operator's home directory that no sandboxed
//! process may read. The list names other tools' login and transcript
//! directories (including other coding agents') only to deny access to them;
//! it is the one place outside the evaluation lanes that the gate's
//! provenance check allows to name them.

/// Where the operator's credentials live, relative to the home directory:
/// keys, cloud and registry tokens, git and HTTP passwords, coding agents'
/// logins, database passwords, shell histories and startup files (tokens
/// typed or exported there), browser profiles (cookies, saved logins). The
/// agent denies them to every command ([`home_secrets`]): a command's output
/// may reach the frontier, and with the egress proxy a command could send
/// what it reads to a package registry, so it must not be able to read them.
/// A list of well-known places, not a guarantee: the rest of the home
/// directory stays readable.
pub const HOME_SECRETS: &[&str] = &[
    // Keys and keyrings.
    ".ssh",
    ".gnupg",
    ".password-store",
    ".local/share/keyrings",
    "Library/Keychains",
    // Clouds, clusters, containers, infrastructure.
    ".aws",
    ".azure",
    ".config/gcloud",
    ".kube",
    ".oci",
    ".config/doctl",
    ".docker/config.json",
    ".terraform.d/credentials.tfrc.json",
    ".vault-token",
    ".fly",
    // Git and HTTP credentials.
    ".netrc",
    ".git-credentials",
    ".config/git/credentials",
    ".config/gh",
    ".config/hub",
    // Package registries.
    ".npmrc",
    ".yarnrc",
    ".yarnrc.yml",
    ".pypirc",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".gem/credentials",
    ".m2/settings.xml",
    ".m2/settings-security.xml",
    ".gradle/gradle.properties",
    ".config/configstore",
    // Coding agents' logins and transcripts.
    ".claude",
    ".claude.json",
    ".codex",
    ".gemini",
    ".config/github-copilot",
    ".local/share/opencode",
    ".declass-eval",
    // Databases and password managers.
    ".pgpass",
    ".my.cnf",
    ".config/op",
    ".op",
    // Shell histories and startup files.
    ".bash_history",
    ".zsh_history",
    ".zhistory",
    ".zsh_sessions",
    ".local/share/fish/fish_history",
    ".python_history",
    ".node_repl_history",
    ".psql_history",
    ".mysql_history",
    ".sqlite_history",
    ".lesshst",
    ".bashrc",
    ".bash_profile",
    ".profile",
    ".zshrc",
    ".zprofile",
    ".config/fish/config.fish",
    // Browser profiles.
    ".mozilla",
    ".config/google-chrome",
    ".config/chromium",
    ".config/BraveSoftware",
    ".config/microsoft-edge",
    "Library/Application Support/Google/Chrome",
    "Library/Application Support/Firefox",
    "Library/Application Support/BraveSoftware",
    "Library/Application Support/Microsoft Edge",
    "Library/Cookies",
];
