// SPDX-License-Identifier: GPL-3.0-or-later
//! Judge adapters for logged-in command-line assistants (measurement only).
//! They live beside the lanes because they launch other tools as black boxes.

use crate::judge::{RUBRIC, Scores, prompt_text, scores_schema};
use anyhow::{Context, Result, bail, ensure};

pub enum JudgeCli {
    /// `claude -p` on the operator's Claude subscription.
    Claude { model: String },
    /// `codex exec` on the operator's ChatGPT subscription.
    Codex { model: Option<String> },
}

impl JudgeCli {
    pub fn parse(backend: &str, model: &str) -> Result<Self> {
        Ok(match backend {
            "claude-cli" => Self::Claude {
                model: model.to_owned(),
            },
            "codex-cli" => Self::Codex {
                model: (!model.starts_with("claude")).then(|| model.to_owned()),
            },
            other => bail!("unknown judge backend {other}"),
        })
    }

    pub fn describe(&self) -> (String, String) {
        match self {
            Self::Claude { model } => ("claude-cli".into(), model.clone()),
            Self::Codex { model } => (
                "codex-cli".into(),
                model.clone().unwrap_or_else(|| "default".into()),
            ),
        }
    }

    /// One judgement: scores and the list-price cost the tool reported.
    pub async fn once(&self, objective: &str, diff: &str) -> Result<(Scores, f64)> {
        match self {
            Self::Claude { model } => claude_once(model, objective, diff).await,
            Self::Codex { model } => codex_once(model.as_deref(), objective, diff).await,
        }
    }
}

/// An empty directory so the judge can see nothing but the prompt.
fn isolated_dir() -> Result<std::path::PathBuf> {
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

async fn claude_once(model: &str, objective: &str, diff: &str) -> Result<(Scores, f64)> {
    let dir = isolated_dir()?;
    let mut cmd = tokio::process::Command::new("claude");
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

async fn codex_once(model: Option<&str>, objective: &str, diff: &str) -> Result<(Scores, f64)> {
    let dir = isolated_dir()?;
    let schema = dir.join("schema.json");
    let answer = dir.join("answer.json");
    std::fs::write(&schema, scores_schema().to_string())?;
    let mut cmd = tokio::process::Command::new("codex");
    cmd.current_dir(&dir).args([
        "exec",
        "--ephemeral",
        "--skip-git-repo-check",
        "-s",
        "read-only",
        "--output-schema",
    ]);
    cmd.arg(&schema).arg("-o").arg(&answer);
    if let Some(m) = model {
        cmd.args(["-m", m]);
    }
    cmd.arg(format!("{RUBRIC}\n\n{}", prompt_text(objective, diff)));
    let result = run(cmd, "")
        .await
        .and_then(|_| Ok(std::fs::read_to_string(&answer)?));
    let _ = std::fs::remove_dir_all(&dir);
    let scores: Scores =
        serde_json::from_str(result?.trim()).context("judge returned no structured scores")?;
    Ok((scores, 0.0))
}
