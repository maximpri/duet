// SPDX-License-Identifier: GPL-3.0-or-later
//! What a run or session uses in place of its configuration, checked against
//! it before the run starts: the mode (`--mode passthrough` needs
//! `frontier.allow_passthrough`), command-line overrides (`--frontier-url`,
//! `--frontier-model`), the values a resumed run recorded when it started,
//! and the local server the bootstrap found. The policy layer, when the
//! configuration has one (`duet_config::policy`), bounds every one of them
//! like it bounds the files; without one only the mode is checked.

use crate::{Mode, RunManifest};
use anyhow::{Result, bail};
use duet_config::{Config, Origin};
use toml::Value;

/// Refuses `--mode passthrough` when `frontier.allow_passthrough` is off,
/// saying who turned it off.
pub(crate) fn mode_allowed(cfg: &Config, mode: Mode) -> Result<()> {
    if mode != Mode::Passthrough || cfg.bool("frontier.allow_passthrough")? {
        return Ok(());
    }
    let by = match cfg.origin("frontier.allow_passthrough") {
        Some(Origin::Project) => "by this repository's .duet/config.toml".to_owned(),
        Some(Origin::Owner) => "in your config".to_owned(),
        Some(Origin::Policy) => format!(
            "by the policy {}",
            cfg.policy()
                .map(|p| p.meta().to_string())
                .unwrap_or_default()
        ),
        Some(Origin::Default) | None => String::new(),
    };
    bail!(
        "--mode passthrough is not allowed here: frontier.allow_passthrough is off {by} \
(passthrough sends everything the model reads to the frontier unfiltered); use --mode hybrid"
    )
}

/// Refuses a run or session whose mode, endpoints or models the
/// configuration does not allow (see the module documentation). Checked for
/// new and resumed runs and sessions alike.
pub(crate) fn check(cfg: &Config, m: &RunManifest) -> Result<()> {
    mode_allowed(cfg, m.mode)?;
    let text = |s: &str| Value::String(s.to_owned());
    if m.mode != Mode::LocalOnly {
        cfg.allows("frontier.base_url", &text(&m.frontier_url))?;
        cfg.allows("frontier.model", &text(&m.frontier_model))?;
        if let Some(d) = &m.frontier_dialect {
            cfg.allows("frontier.dialect", &text(d))?;
        }
    }
    if let Some(local) = &m.local {
        cfg.allows("local.base_url", &text(&local.base_url))?;
        cfg.allows("local.model", &text(&local.model))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalOverride;
    use duet_config::{PolicyMeta, PolicySource};

    struct Text(&'static str);

    impl PolicySource for Text {
        fn load(&self) -> Result<Option<(String, PolicyMeta)>, duet_config::PolicyError> {
            Ok(Some((
                self.0.into(),
                PolicyMeta::new("test", "not verified"),
            )))
        }
    }

    const POLICY: &str = "[policy]\nname = \"acme\"\n\
        [frontier]\nbase_url = \"https://gateway.acme.example/v1\"\nmodel = \"approved\"\n\
        allow_passthrough = false\n\
        [local]\nbase_url = \"http://127.0.0.1:8080/v1\"\n";

    fn config(
        owner: &str,
        project: &str,
        policy: Option<&dyn PolicySource>,
    ) -> (tempfile::TempDir, Config) {
        let d = tempfile::tempdir().unwrap();
        let (o, p) = (d.path().join("owner.toml"), d.path().join("project.toml"));
        std::fs::write(&o, owner).unwrap();
        std::fs::write(&p, project).unwrap();
        let cfg = Config::load_with(&o, Some(&p), policy).unwrap();
        (d, cfg)
    }

    fn manifest(cfg: &Config, mode: Mode) -> RunManifest {
        RunManifest {
            run_id: "r".into(),
            mode,
            objective: "task".into(),
            frontier_url: cfg.str("frontier.base_url").unwrap(),
            frontier_model: cfg.str("frontier.model").unwrap(),
            frontier_dialect: Some(cfg.str("frontier.dialect").unwrap()),
            local: None,
            session: false,
            images: Vec::new(),
        }
    }

    #[test]
    fn passthrough_needs_the_setting_and_says_who_turned_it_off() {
        let (_d, open) = config("", "", None);
        assert!(check(&open, &manifest(&open, Mode::Passthrough)).is_ok());
        let (_d, project) = config("", "[frontier]\nallow_passthrough = false\n", None);
        let e = check(&project, &manifest(&project, Mode::Passthrough)).unwrap_err();
        assert!(e.to_string().contains(".duet/config.toml"), "{e}");
        assert!(check(&project, &manifest(&project, Mode::Hybrid)).is_ok());
        // The owner turning it back on does not override a project's off.
        let (_d, both) = config(
            "[frontier]\nallow_passthrough = true\n",
            "[frontier]\nallow_passthrough = false\n",
            None,
        );
        assert!(check(&both, &manifest(&both, Mode::Passthrough)).is_err());
        let (_d, policed) = config(
            "[frontier]\nallow_passthrough = true\n",
            "",
            Some(&Text(POLICY)),
        );
        let e = mode_allowed(&policed, Mode::Passthrough).unwrap_err();
        assert!(e.to_string().contains("by the policy acme"), "{e}");
    }

    #[test]
    fn overrides_and_recorded_values_are_bounded_by_the_policy() {
        let (_d, cfg) = config(
            "[frontier]\nbase_url = \"https://elsewhere.example/v1\"\n",
            "",
            Some(&Text(POLICY)),
        );
        let ok = manifest(&cfg, Mode::Hybrid);
        assert_eq!(ok.frontier_url, "https://gateway.acme.example/v1");
        check(&cfg, &ok).unwrap();
        // --frontier-url, --frontier-model, or a run recorded before the
        // policy (resumed): refused.
        for m in [
            RunManifest {
                frontier_url: "https://elsewhere.example/v1".into(),
                ..manifest(&cfg, Mode::Hybrid)
            },
            RunManifest {
                frontier_model: "unapproved".into(),
                ..manifest(&cfg, Mode::Hybrid)
            },
            // The bootstrap's local server (DUET_LOCAL_PORTS picks where it looks).
            RunManifest {
                local: Some(LocalOverride {
                    base_url: "http://127.0.0.1:11434/v1".into(),
                    model: "qwen".into(),
                }),
                ..manifest(&cfg, Mode::Hybrid)
            },
            manifest(&cfg, Mode::Passthrough),
        ] {
            let e = check(&cfg, &m).unwrap_err();
            assert!(e.to_string().contains("acme"), "{e}");
        }
        // Local-only mode never reaches the frontier: its recorded endpoint is not checked.
        check(
            &cfg,
            &RunManifest {
                frontier_url: "https://elsewhere.example/v1".into(),
                ..manifest(&cfg, Mode::LocalOnly)
            },
        )
        .unwrap();
        // Without a policy every override is the operator's.
        let (_d, plain) = config("", "", None);
        check(
            &plain,
            &RunManifest {
                frontier_url: "https://elsewhere.example/v1".into(),
                ..manifest(&plain, Mode::Hybrid)
            },
        )
        .unwrap();
    }

    #[test]
    fn presets_cannot_change_what_the_policy_fixes() {
        let (_d, mut cfg) = config("", "", Some(&Text(POLICY)));
        let e = crate::setup::preset(&mut cfg, Some("anthropic"), None, None, true).unwrap_err();
        assert!(e.to_string().contains("acme"), "{e}");
        let e = crate::setup::preset(&mut cfg, Some("ollama"), None, None, true).unwrap_err();
        assert!(e.to_string().contains("local.base_url"), "{e}");
        assert_eq!(std::fs::read_to_string(&cfg.owner_path).unwrap(), "");
    }
}
