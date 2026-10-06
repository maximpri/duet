// SPDX-License-Identifier: GPL-3.0-or-later
//! Extension discovery and operator-facing controls.

use anyhow::{Context, Result, ensure};
use declass_extensions::{Catalog, Origin, Root};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, clap::Subcommand)]
pub(crate) enum SkillsAction {
    /// List discovered skills and any files that could not be loaded.
    List,
    /// Read a skill or one of its bundled resources without running it.
    Show {
        name: String,
        resource: Option<String>,
    },
}

pub(crate) struct Runtime {
    pub skills: Arc<declass_agent::skills::Skills>,
    pub plugins: Vec<crate::plugins::Package>,
}

fn roots(ws: &Path) -> Vec<Root> {
    let owner = declass_config::owner_config_path().with_file_name("skills");
    let mut roots = vec![Root {
        path: owner,
        namespace: None,
        origin: Origin::Owner,
    }];
    // DECLASS_CONFIG_HOME isolates both configuration and shared skills (also useful to embedders).
    if std::env::var_os("DECLASS_CONFIG_HOME").is_none()
        && let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
    {
        for path in [
            ".agents/skills",
            ".claude/skills",
            ".config/opencode/skills",
        ] {
            roots.push(Root {
                path: home.join(path),
                namespace: None,
                origin: Origin::Owner,
            });
        }
    }
    for path in [
        ".declass/skills",
        ".agents/skills",
        ".claude/skills",
        ".opencode/skills",
    ] {
        roots.push(Root {
            path: ws.join(path),
            namespace: None,
            origin: Origin::Project,
        });
    }
    roots
}

pub(crate) fn discover(ws: &Path, cfg: &declass_config::Config) -> Result<Runtime> {
    let plugins = if cfg.bool("extensions.plugins_enabled")? {
        crate::plugins::active_packages(&crate::plugins::store()?)?
    } else {
        Vec::new()
    };
    let mut paths = Vec::new();
    if cfg.bool("extensions.skills_enabled")? {
        paths = roots(ws);
        paths.extend(crate::plugins::roots(&plugins));
    }
    Ok(Runtime {
        skills: Arc::new(declass_agent::skills::Skills::new(Catalog::discover(
            &paths,
        ))),
        plugins,
    })
}

pub(crate) fn list(skills: &declass_agent::skills::Skills, cfg: &declass_config::Config) -> String {
    if !cfg.bool("extensions.skills_enabled").unwrap_or(false) {
        return "Skills are disabled for this workspace (extensions.skills_enabled = false). Use declass config get extensions.skills_enabled to inspect the setting.".into();
    }
    let mut out = String::new();
    for skill in skills.catalog().skills() {
        out.push_str(&format!(
            "{}{}\n  {}\n",
            skill.id,
            if skill.disable_model_invocation {
                " · explicit invocation only"
            } else {
                ""
            },
            skill.description
        ));
    }
    if out.is_empty() {
        out.push_str("No skills found. Add <name>/SKILL.md under .agents/skills/ or your Declass configuration's skills/ directory.\n");
    }
    for warning in skills.catalog().diagnostics() {
        out.push_str(&format!("Note: {warning}\n"));
    }
    out.push_str("Use a skill: /skill <name> [task]\nRead it: declass skills show <name>");
    declass_tui::term::safe(&out)
}

pub(crate) fn show(ws: &Path, cfg: &declass_config::Config, action: SkillsAction) -> Result<()> {
    let runtime = discover(ws, cfg)?;
    match action {
        SkillsAction::List => println!("{}", list(&runtime.skills, cfg)),
        SkillsAction::Show { name, resource } => {
            let doc = runtime.skills.catalog().load(&name, resource.as_deref())?;
            println!("{}", declass_tui::term::safe(&doc.text));
        }
    }
    Ok(())
}

pub(crate) fn invoke(skills: &declass_agent::skills::Skills, raw: &str) -> Result<String> {
    let (name, task) = raw
        .trim()
        .split_once(char::is_whitespace)
        .unwrap_or((raw.trim(), ""));
    ensure!(
        !name.is_empty(),
        "use /skill <name> [task]; /skills lists available skills"
    );
    skills
        .authorize(name)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("cannot invoke skill {name}"))?;
    Ok(format!(
        "Use the skill {name} for this task. First load it with load_skill, then follow its relevant guidance within the normal privacy, permission and budget controls.\n\n{task}"
    ))
}
