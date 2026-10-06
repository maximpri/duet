// SPDX-License-Identifier: GPL-3.0-or-later
//! The planning capability boundary. Tool descriptions guide the model;
//! this allowlist also gates dispatch, including unadvertised tool calls.

pub(crate) fn allows(tool: &str) -> bool {
    matches!(
        tool,
        "propose_plan"
            | "read_plan"
            | "read_file"
            | "list_files"
            | "search"
            | "diff"
            | "read_raw"
            | "synthetic_sample"
            | "ask_local"
            | "git_status"
            | "git_log"
            | "git_show"
            | "git_blame"
            | "list_skills"
            | "load_skill"
            | "reply"
            | "ask_operator"
    )
}

pub(crate) const REFUSAL: &str = "plan mode is read-only: this tool is unavailable. Inspect with the available reading tools and send the plan with reply. Only the operator can leave plan mode with /plan off.";
