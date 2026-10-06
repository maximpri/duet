// SPDX-License-Identifier: GPL-3.0-or-later
//! Operator-facing pending attachments. IDs refer to the current queue; they
//! never address files in the workspace or attachments already in a transcript.

use super::{Command, GoalAction, Screen};
use crate::attachments::Attachment;
use crate::images::AttachedImage;
use anyhow::{Result, bail};
use declass_tui::workspace::AttachmentChip;
use std::path::Path;

pub(super) enum Selection {
    All,
    Text(usize),
    Image(usize),
}

pub(super) fn requests_turn(command: &Command) -> bool {
    matches!(
        command,
        Command::Message(_) | Command::Skill(_) | Command::PluginPrompt(_) | Command::Answer(_)
    ) || matches!(command, Command::Plan(raw) if matches!(super::plan_action(raw), super::PlanAction::Task(_) | super::PlanAction::Revise(_) | super::PlanAction::Implement(_) | super::PlanAction::Resume(_)))
        || matches!(command, Command::Goal(raw) if matches!(super::goal_action(raw), GoalAction::Start(_) | GoalAction::Resume))
}

pub(super) fn plan_requests_turn(action: &declass_tui::workspace::PlanAction) -> bool {
    matches!(
        action,
        declass_tui::workspace::PlanAction::Implement { .. }
            | declass_tui::workspace::PlanAction::Resume { .. }
            | declass_tui::workspace::PlanAction::Revise { .. }
            | declass_tui::workspace::PlanAction::Answer { .. }
    )
}

/// Keep a queued request for operator review if its preceding image failed.
/// Later independent attachments/requests retain their original ordering.
pub(super) fn recover_dependent(held: &mut Vec<String>) -> Option<String> {
    let index = held
        .iter()
        .position(|line| requests_turn(&super::parse(line)))?;
    Some(held.remove(index))
}

pub(super) fn selection(raw: &str) -> Result<Selection> {
    let raw = raw.trim().to_ascii_lowercase();
    if raw == "all" {
        return Ok(Selection::All);
    }
    let (kind, number) = raw.split_at_checked(1).unwrap_or(("", ""));
    if let Ok(number) = number.parse::<usize>()
        && let Some(index) = number.checked_sub(1)
    {
        return match kind {
            "t" => Ok(Selection::Text(index)),
            "i" => Ok(Selection::Image(index)),
            _ => Err(anyhow::anyhow!(
                "use /detach t1, /detach i1 or /detach all; /attachments shows current IDs"
            )),
        };
    }
    bail!("use /detach t1, /detach i1 or /detach all; /attachments shows current IDs")
}

pub(super) fn chips<'a>(
    files: &[Attachment],
    images: impl IntoIterator<Item = (&'a Path, bool)>,
) -> Vec<AttachmentChip> {
    let mut chips: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(i, file)| AttachmentChip {
            id: format!("t{}", i + 1),
            label: declass_tui::term::safe(&file.label()),
        })
        .collect();
    chips.extend(
        images
            .into_iter()
            .enumerate()
            .map(|(i, (path, public))| AttachmentChip {
                id: format!("i{}", i + 1),
                label: declass_tui::term::safe(&format!(
                    "{} · image{}",
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy(),
                    if public { " · marked public" } else { "" }
                )),
            }),
    );
    chips
}

pub(super) fn publish(screen: &Screen, chips: Vec<AttachmentChip>) {
    if let Some(workspace) = screen.workspace() {
        workspace.attachments(chips);
    }
}

pub(super) fn summary(chips: &[AttachmentChip]) -> String {
    if chips.is_empty() {
        return "No attachments waiting. Drop a file path, use /attach PATH, or paste an image with Ctrl-V.".into();
    }
    let mut text = "Attachments for your next message:\n".to_owned();
    for chip in chips {
        text.push_str(&format!("  {}  {}\n", chip.id, chip.label));
    }
    text.push_str("Remove with /detach ID or /detach all. IDs update after removal.");
    text
}

pub(super) fn detach(
    files: &mut Vec<Attachment>,
    images: &mut Vec<AttachedImage>,
    raw: &str,
) -> Result<String> {
    match selection(raw)? {
        Selection::All => {
            let count = files.len() + images.len();
            files.clear();
            images.clear();
            Ok(format!("Removed {count} pending attachment(s)."))
        }
        Selection::Text(index) if index < files.len() => {
            files.remove(index);
            Ok("Removed text attachment. /attachments shows the remaining IDs.".into())
        }
        Selection::Image(index) if index < images.len() => {
            images.remove(index);
            Ok("Removed image attachment. /attachments shows the remaining IDs.".into())
        }
        _ => bail!("No pending attachment with that ID; /attachments shows current IDs."),
    }
}

pub(super) fn detach_session(
    files: &mut Vec<Attachment>,
    session: &mut declass_agent::Session<'_>,
    raw: &str,
) -> Result<String> {
    match selection(raw)? {
        Selection::All => {
            let count = files.len() + session.clear_images();
            files.clear();
            Ok(format!("Removed {count} pending attachment(s)."))
        }
        Selection::Text(index) if index < files.len() => {
            files.remove(index);
            Ok("Removed text attachment. /attachments shows the remaining IDs.".into())
        }
        Selection::Image(index) if session.detach_image(index) => {
            Ok("Removed image attachment. /attachments shows the remaining IDs.".into())
        }
        _ => bail!("No pending attachment with that ID; /attachments shows current IDs."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_attachments_can_be_listed_and_removed_without_touching_files() {
        let d = tempfile::tempdir().unwrap();
        let original = d.path().join("notes.md");
        std::fs::write(&original, "notes").unwrap();
        let mut files = Vec::new();
        crate::attachments::add(&mut files, d.path(), original.to_str().unwrap()).unwrap();
        let mut images = vec![AttachedImage {
            path: d.path().join("shot.png"),
            public: false,
        }];
        let listed = chips(&files, images.iter().map(|i| (i.path.as_path(), i.public)));
        assert_eq!(
            listed.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["t1", "i1"]
        );
        assert!(summary(&listed).contains("notes.md"));
        assert!(detach(&mut files, &mut images, "i0").is_err());
        assert!(detach(&mut files, &mut images, "i2").is_err());
        detach(&mut files, &mut images, "t1").unwrap();
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "notes");
        assert!(files.is_empty());
        assert_eq!(images.len(), 1);
        detach(&mut files, &mut images, "all").unwrap();
        assert!(images.is_empty());
    }

    #[test]
    fn failed_image_recovers_its_request_without_reordering_later_attachments() {
        let mut held = vec![
            "/status".into(),
            "Inspect first image".into(),
            "/image second.png".into(),
            "Inspect second image".into(),
            "/quit".into(),
        ];
        assert_eq!(
            recover_dependent(&mut held).as_deref(),
            Some("Inspect first image")
        );
        assert_eq!(
            held,
            [
                "/status",
                "/image second.png",
                "Inspect second image",
                "/quit"
            ]
        );
        assert!(requests_turn(&Command::Skill("review screenshot".into())));
        assert!(requests_turn(&Command::Goal("resume".into())));
        assert!(!requests_turn(&Command::Goal("pause".into())));
    }
}
