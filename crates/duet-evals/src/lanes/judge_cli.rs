// SPDX-License-Identifier: GPL-3.0-or-later
//! Judge adapters for logged-in command-line assistants (measurement only).
//! They live beside the lanes because they launch other tools as black boxes.
//!
//! Every run is judged by two judges of different model families (PLAN §3,
//! 2026-09-24), each on the operator's own login and never with an API key.
//! Each judge's result is stored in its own file in the run directory.

use crate::judge::{RUBRIC, Scores, prompt_text, scores_schema};
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};

/// A judge: its storage name, how it is reached and its model family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JudgeSpec {
    /// Storage name: the judgement lives in `judge-<name>.json`.
    pub name: &'static str,
    /// The `--judges` value.
    pub backend: &'static str,
    /// Vendor family, compared with a lane's family to flag self-judging.
    pub family: &'static str,
    pub default_model: &'static str,
}

pub const ANTHROPIC_JUDGE: JudgeSpec = JudgeSpec {
    name: "claude",
    backend: "claude-cli",
    family: "anthropic",
    default_model: "claude-opus-5-5",
};

pub const OPENAI_JUDGE: JudgeSpec = JudgeSpec {
    name: "codex",
    backend: "codex-cli",
    family: "openai",
    default_model: "gpt-5.5",
};

/// The Anthropic API with `ANTHROPIC_API_KEY`; the same judge as [`ANTHROPIC_JUDGE`].
pub const API_JUDGE: JudgeSpec = JudgeSpec {
    name: "claude",
    backend: "api",
    family: "anthropic",
    default_model: "claude-opus-5-5",
};

pub const SPECS: &[JudgeSpec] = &[ANTHROPIC_JUDGE, OPENAI_JUDGE, API_JUDGE];

/// `duet-eval judge` runs both families unless told otherwise.
pub const DEFAULT_JUDGES: &str = "claude-cli,codex-cli";

/// Judges every run must have for the public benchmark (`report --final`).
pub const FINAL_JUDGES: &[&str] = &[ANTHROPIC_JUDGE.name, OPENAI_JUDGE.name];

/// Judgements written before there were two judges; read as the judge its
/// `backend` field names (in practice the Claude judge).
pub const LEGACY_FILE: &str = "judge.json";

pub fn file_name(judge: &str) -> String {
    format!("judge-{judge}.json")
}

pub fn spec_for_backend(backend: &str) -> Result<JudgeSpec> {
    SPECS
        .iter()
        .find(|s| s.backend == backend)
        .copied()
        .with_context(|| {
            format!(
                "unknown judge backend {backend} (known: {})",
                SPECS
                    .iter()
                    .map(|s| s.backend)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// The judge a stored judgement belongs to, from its recorded backend. Files
/// from before the backend was recorded are the Claude judge's.
pub fn spec_for_judgement(backend: &str) -> JudgeSpec {
    spec_for_backend(backend).unwrap_or(ANTHROPIC_JUDGE)
}

/// The model each judge uses: its default unless `overrides` (`backend=model`) names one.
pub fn model_for(spec: &JudgeSpec, overrides: &[String]) -> Result<String> {
    for o in overrides {
        let (backend, model) = o
            .split_once('=')
            .with_context(|| format!("--model must be backend=model, got {o}"))?;
        spec_for_backend(backend)?;
        if backend == spec.backend {
            ensure!(!model.trim().is_empty(), "empty model for {backend}");
            return Ok(model.trim().to_owned());
        }
    }
    Ok(spec.default_model.to_owned())
}

pub enum JudgeCli {
    /// `claude -p` on the operator's Claude login.
    Claude {
        model: String,
        version: Option<String>,
    },
    /// `codex exec` on the operator's normal ChatGPT login.
    Codex {
        model: String,
        version: Option<String>,
    },
}

impl JudgeCli {
    pub fn new(spec: &JudgeSpec, model: &str) -> Result<Self> {
        Ok(match spec.backend {
            "claude-cli" => Self::Claude {
                model: model.to_owned(),
                version: cli_version("claude"),
            },
            "codex-cli" => Self::Codex {
                model: model.to_owned(),
                version: cli_version("codex"),
            },
            other => bail!("{other} is not a command-line judge"),
        })
    }

    pub fn spec(&self) -> JudgeSpec {
        match self {
            Self::Claude { .. } => ANTHROPIC_JUDGE,
            Self::Codex { .. } => OPENAI_JUDGE,
        }
    }

    pub fn model(&self) -> &str {
        match self {
            Self::Claude { model, .. } | Self::Codex { model, .. } => model,
        }
    }

    pub fn version(&self) -> Option<String> {
        match self {
            Self::Claude { version, .. } | Self::Codex { version, .. } => version.clone(),
        }
    }

    /// One judgement: scores and the list-price cost the tool reported.
    pub async fn once(&self, objective: &str, diff: &str) -> Result<(Scores, f64)> {
        match self {
            Self::Claude { model, .. } => claude_once(model, objective, diff).await,
            Self::Codex { model, .. } => codex_once(model, objective, diff).await,
        }
    }
}

/// First line of `<program> --version`, recorded in every judgement.
fn cli_version(program: &str) -> Option<String> {
    let out = std::process::Command::new(program)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned()
    })
}

/// An empty directory so the judge can see nothing but the prompt.
fn isolated_dir() -> Result<PathBuf> {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("duet-judge-{n:x}-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

async fn run(mut cmd: tokio::process::Command, stdin: &str) -> Result<Vec<u8>> {
    use tokio::io::AsyncWriteExt;
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().context("starting the judge CLI")?;
    if let Some(mut pipe) = child.stdin.take() {
        pipe.write_all(stdin.as_bytes()).await?;
    }
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(900),
        child.wait_with_output(),
    )
    .await
    .context("judge CLI timed out")??;
    let tail: String = String::from_utf8_lossy(&out.stdout)
        .chars()
        .rev()
        .take(800)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    ensure!(
        out.status.success(),
        "judge CLI failed (exit {:?}): {} {tail}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
            .chars()
            .take(500)
            .collect::<String>()
    );
    Ok(out.stdout)
}

/// Settings that would send a judge through a proxy or bill an API key instead
/// of the operator's login.
const ROUTING_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_BASE_URL",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
];

async fn claude_once(model: &str, objective: &str, diff: &str) -> Result<(Scores, f64)> {
    let dir = isolated_dir()?;
    let mut cmd = tokio::process::Command::new("claude");
    for k in ROUTING_ENV {
        cmd.env_remove(k);
    }
    cmd.current_dir(&dir).args([
        "-p",
        "--output-format",
        "json",
        "--model",
        model,
        "--no-session-persistence",
        "--system-prompt",
        RUBRIC,
        "--json-schema",
        &scores_schema().to_string(),
    ]);
    let out = run(cmd, &prompt_text(objective, diff)).await;
    let _ = std::fs::remove_dir_all(&dir);
    let v: serde_json::Value = serde_json::from_slice(&out?).context("judge output is not JSON")?;
    ensure!(
        v["is_error"] != true,
        "judge reported an error: {}",
        v["result"]
    );
    let scores: Scores = serde_json::from_value(v["structured_output"].clone())
        .context("judge returned no structured scores")?;
    Ok((scores, v["total_cost_usd"].as_f64().unwrap_or(0.0)))
}

/// The Codex home the judge uses: the operator's normal one. The dedicated
/// evaluation login (`~/.duet-eval/codex`) belongs to the benchmark lane, so a
/// `CODEX_HOME` pointing there is dropped and the default home applies.
pub fn judge_codex_home(env_home: Option<&Path>, operator_home: &Path) -> Option<PathBuf> {
    let eval_home = operator_home.join(".duet-eval").join("codex");
    let same = |p: &Path| {
        let canon = |q: &Path| std::fs::canonicalize(q).unwrap_or_else(|_| q.to_path_buf());
        canon(p) == canon(&eval_home) || p == eval_home
    };
    match env_home.filter(|p| !p.as_os_str().is_empty()) {
        Some(p) if !same(p) => Some(p.to_path_buf()),
        _ => None,
    }
}

/// Codex answers in the final message; `--output-schema` constrains its shape
/// and `-o` writes it to a file, so the event stream is never parsed.
async fn codex_once(model: &str, objective: &str, diff: &str) -> Result<(Scores, f64)> {
    let dir = isolated_dir()?;
    let work = dir.join("work");
    std::fs::create_dir_all(&work)?;
    let schema = dir.join("schema.json");
    let answer = dir.join("answer.json");
    std::fs::write(&schema, scores_schema().to_string())?;
    let mut cmd = tokio::process::Command::new("codex");
    for k in ROUTING_ENV {
        cmd.env_remove(k);
    }
    let operator_home = crate::lanes::operator_home()?;
    let env_home = std::env::var_os("CODEX_HOME").map(PathBuf::from);
    match judge_codex_home(env_home.as_deref(), &operator_home) {
        Some(home) => cmd.env("CODEX_HOME", home),
        None => cmd.env_remove("CODEX_HOME"),
    };
    cmd.current_dir(&work).args([
        "exec",
        "--ephemeral",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "--ignore-rules",
        "--color",
        "never",
        "-s",
        "read-only",
        "-m",
        model,
        "-C",
    ]);
    cmd.arg(&work)
        .arg("--output-schema")
        .arg(&schema)
        .arg("-o")
        .arg(&answer)
        // The prompt comes from stdin: diffs can exceed an argument's size limit.
        .arg("-");
    let prompt = format!(
        "{RUBRIC}\n\nAnswer with a single JSON object matching the provided schema and nothing else.\n\n{}",
        prompt_text(objective, diff)
    );
    let result = run(cmd, &prompt)
        .await
        .and_then(|_| Ok(std::fs::read_to_string(&answer)?));
    let _ = std::fs::remove_dir_all(&dir);
    Ok((parse_json_answer(&result?)?, 0.0))
}

/// Scores from a JSON-only answer, tolerating a Markdown code fence around it.
pub fn parse_json_answer(text: &str) -> Result<Scores> {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .and_then(|rest| rest.trim_end().strip_suffix("```"))
        .unwrap_or(t)
        .trim();
    serde_json::from_str(t).context("judge returned no structured scores")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judges_are_two_families_and_the_api_is_the_claude_judge() {
        assert_ne!(ANTHROPIC_JUDGE.family, OPENAI_JUDGE.family);
        assert_eq!(API_JUDGE.name, ANTHROPIC_JUDGE.name);
        assert_eq!(spec_for_judgement("api"), API_JUDGE);
        assert_eq!(spec_for_judgement("codex-cli"), OPENAI_JUDGE);
        assert_eq!(spec_for_judgement(""), ANTHROPIC_JUDGE);
        assert!(spec_for_backend("other").is_err());
        let defaults: Vec<_> = DEFAULT_JUDGES.split(',').collect();
        assert_eq!(defaults, [ANTHROPIC_JUDGE.backend, OPENAI_JUDGE.backend]);
    }

    #[test]
    fn models_default_per_judge_and_can_be_overridden() {
        let o = vec!["codex-cli=gpt-x".to_owned()];
        assert_eq!(model_for(&OPENAI_JUDGE, &o).unwrap(), "gpt-x");
        assert_eq!(
            model_for(&ANTHROPIC_JUDGE, &o).unwrap(),
            ANTHROPIC_JUDGE.default_model
        );
        assert!(model_for(&ANTHROPIC_JUDGE, &["nope".into()]).is_err());
        assert!(model_for(&ANTHROPIC_JUDGE, &["other=x".into()]).is_err());
    }

    #[test]
    fn the_judge_never_uses_the_evaluation_codex_home() {
        let home = Path::new("/home/op");
        assert_eq!(judge_codex_home(None, home), None);
        assert_eq!(
            judge_codex_home(Some(Path::new("/home/op/.duet-eval/codex")), home),
            None
        );
        assert_eq!(
            judge_codex_home(Some(Path::new("/srv/codex")), home),
            Some(PathBuf::from("/srv/codex"))
        );
    }

    #[test]
    fn json_answers_parse_with_or_without_a_fence() {
        let body =
            r#"{"correctness_risk":8,"maintainability":7,"scope_discipline":9,"deductions":[]}"#;
        assert_eq!(parse_json_answer(body).unwrap().total(), 24);
        assert_eq!(
            parse_json_answer(&format!("```json\n{body}\n```"))
                .unwrap()
                .total(),
            24
        );
        assert!(parse_json_answer("I think it is fine.").is_err());
    }
}
