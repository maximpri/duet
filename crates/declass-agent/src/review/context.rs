// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only contextual references, always read back from the pinned snapshot.
use super::{Snapshot, extension};
use declass_boundary::view::Presenter;
use declass_review::Candidate;
use serde_json::json;
use std::path::{Path, PathBuf};

pub(super) async fn gather(
    workspace: &Path,
    snapshot: &Snapshot,
    path: &Path,
    candidate: &Candidate,
    lsp: &declass_lsp::Lsp,
    presenter: &dyn Presenter,
    stopped: &(impl Fn() -> bool + Sync),
) -> Vec<(PathBuf, String)> {
    let Some(source) = snapshot.files.get(path).and_then(|f| f.source.as_deref()) else {
        return vec![];
    };
    // Match ordinary navigation: never send sensitive/protected documents to
    // a language server. Its local process may resolve protected definitions.
    if presenter.path_sensitive(path)
        || presenter.protection(path).is_some()
        || !presenter.review_value_spans(source).is_empty()
    {
        return vec![];
    }
    let hidden = || {
        let mut paths = presenter.hidden_from_checks(workspace);
        paths.extend([workspace.join(".declass"), workspace.join(".git")]);
        paths
    };
    let mut out = Vec::new();
    for (line, column) in
        declass_review::context::references(extension(path), source, candidate.line)
            .into_iter()
            .take(2)
    {
        if stopped() {
            break;
        }
        let Ok(position) = declass_lsp::position::to_lsp(source, line as u32, column as u32) else {
            continue;
        };
        for method in ["textDocument/definition", "textDocument/references"] {
            if stopped() {
                return out;
            }
            let params = |uri: &str| json!({"textDocument":{"uri":uri}, "position":position, "context":{"includeDeclaration":true}});
            let answer = tokio::select! {
                result = lsp.request(path, source, &hidden, method, &params) => result,
                () = async {
                    while !stopped() {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                } => return out,
            };
            let Ok(answer) = answer else { continue };
            let values = answer.as_array().cloned().unwrap_or_else(|| vec![answer]);
            for v in values.into_iter().take(4) {
                let Some(uri) = v["uri"].as_str().or_else(|| v["targetUri"].as_str()) else {
                    continue;
                };
                let Some(absolute) = url::Url::parse(uri)
                    .ok()
                    .and_then(|u| u.to_file_path().ok())
                else {
                    continue;
                };
                let Ok(rel) = absolute.strip_prefix(workspace) else {
                    continue;
                };
                if rel == path
                    || declass_fs::is_reserved(rel)
                    || rel
                        .components()
                        .any(|c| !matches!(c, std::path::Component::Normal(_)))
                {
                    continue;
                }
                if out.iter().any(|(p, _)| p == rel) {
                    continue;
                }
                let Some(text) = snapshot.files.get(rel).and_then(|f| f.source.as_deref()) else {
                    continue;
                };
                let range = v.get("range").or_else(|| v.get("targetSelectionRange"));
                let line =
                    range.and_then(|r| r["start"]["line"].as_u64()).unwrap_or(0) as usize + 1;
                let context = declass_review::context::at(extension(rel), text, line);
                out.push((rel.to_path_buf(), context));
                if out.len() >= 4 {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use declass_boundary::view::PassThrough;
    #[tokio::test]
    async fn definitions_are_read_from_the_snapshot_and_private_documents_are_not_sent() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap();
        let source = "fn main() { helper(); client.danger_accept_invalid_certs(true); }";
        std::fs::write(ws.join("main.rs"), source).unwrap();
        std::fs::write(ws.join("helper.rs"), "fn helper() { original(); }").unwrap();
        let snapshot = super::super::snapshot(&ws);
        std::fs::write(ws.join("helper.rs"), "fn helper() { changed(); }").unwrap();
        let lsp = declass_lsp::Lsp::new(
            ws.clone(),
            vec![declass_lsp::mock::detected("rust", &["rs"])],
            Box::new(declass_lsp::mock::MockLauncher::default()),
            declass_lsp::Settings::default(),
        );
        let c = declass_review::scan("rs", source, &[]).candidates.remove(0);
        let refs = gather(
            &ws,
            &snapshot,
            Path::new("main.rs"),
            &c,
            &lsp,
            &PassThrough { max_bytes: 60000 },
            &|| false,
        )
        .await;
        assert!(
            refs.iter().any(|(p, s)| p == Path::new("helper.rs")
                && s.contains("original")
                && !s.contains("changed")),
            "{refs:?}"
        );
        struct Private;
        impl Presenter for Private {
            fn present(&self, _: &declass_boundary::view::Source, _: &[u8]) -> String {
                String::new()
            }
            fn path_sensitive(&self, _: &Path) -> bool {
                true
            }
        }
        assert!(
            gather(
                &ws,
                &snapshot,
                Path::new("main.rs"),
                &c,
                &lsp,
                &Private,
                &|| false
            )
            .await
            .is_empty()
        );
        lsp.shutdown().await;
    }
}
