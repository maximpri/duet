// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded structured clarification. Choices never grant execution permissions.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionOptions {
    /// Set by the host, never by tool arguments.
    pub id: String,
    pub choices: Vec<QuestionOption>,
    #[serde(default = "yes")]
    pub allow_freeform: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<String>,
}
fn yes() -> bool {
    true
}

pub(crate) fn parse(value: Option<&Value>) -> Result<Option<QuestionOptions>, String> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let (choices, allow_freeform, recommended) = if let Some(a) = value.as_array() {
        if a.is_empty() {
            return Ok(None);
        }
        let choices = a
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let label = v.as_str().ok_or("question options must be strings")?;
                Ok(QuestionOption {
                    id: format!("option-{}", i + 1),
                    label: label.to_owned(),
                    description: None,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        (choices, true, None)
    } else {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            choices: Vec<QuestionOption>,
            #[serde(default = "yes")]
            allow_freeform: bool,
            #[serde(default)]
            recommended: Option<String>,
        }
        let input: Input = serde_json::from_value(value.clone())
            .map_err(|_| "invalid structured question options")?;
        (input.choices, input.allow_freeform, input.recommended)
    };
    if !(2..=6).contains(&choices.len()) {
        return Err("provide between two and six choices".into());
    }
    let mut ids = std::collections::HashSet::new();
    for c in &choices {
        if c.id.is_empty()
            || c.id.len() > 64
            || !c
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || !ids.insert(c.id.clone())
        {
            return Err("choice IDs must be unique short ASCII identifiers".into());
        }
        if c.label.trim().is_empty()
            || c.label.len() > 512
            || c.description.as_ref().is_some_and(|s| s.len() > 2048)
        {
            return Err("question choice text is empty or too long".into());
        }
    }
    if recommended.as_ref().is_some_and(|id| !ids.contains(id)) {
        return Err("recommended choice must name a choice ID".into());
    }
    Ok(Some(QuestionOptions {
        id: String::new(),
        choices,
        allow_freeform,
        recommended,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn old_saved_questions_resume_without_structured_metadata() {
        let old = serde_json::json!({"state":"asked","question":"Continue?"});
        let end: crate::session::TurnEnd = serde_json::from_value(old.clone()).unwrap();
        assert!(matches!(
            end,
            crate::session::TurnEnd::Asked { options: None, .. }
        ));
        assert_eq!(serde_json::to_value(end).unwrap(), old);
    }
    #[test]
    fn legacy_choices_remain_structured_without_granting_permissions() {
        let q = parse(Some(&json!(["Yes", "No"]))).unwrap().unwrap();
        assert_eq!(q.choices[0].id, "option-1");
        assert!(q.allow_freeform);
        assert!(q.id.is_empty());
    }
    #[test]
    fn malformed_or_authority_bearing_choices_are_rejected() {
        for v in [
            json!({"choices":[],"approved":true}),
            json!(["Only"]),
            json!(["Yes", false]),
            json!({"choices":[{"id":"x","label":"one"},{"id":"x","label":"two"}]}),
            json!({"id":"forged","choices":[{"id":"a","label":"one"},{"id":"b","label":"two"}]}),
        ] {
            assert!(parse(Some(&v)).is_err());
        }
    }
    #[test]
    fn recommendation_must_reference_a_real_choice() {
        let good = json!({"choices":[{"id":"a","label":"one"},{"id":"b","label":"two"}],"recommended":"a","allow_freeform":false});
        let q = parse(Some(&good)).unwrap().unwrap();
        assert_eq!(q.recommended.as_deref(), Some("a"));
        assert!(!q.allow_freeform);
        let mut bad = good;
        bad["recommended"] = json!("missing");
        assert!(parse(Some(&bad)).is_err());
    }
}
