// SPDX-License-Identifier: GPL-3.0-or-later
//! Where an image may go.
//!
//! Images cannot be scanned: text in a screenshot, a scanned form or a photo
//! of a whiteboard is invisible to the detectors, the vault and the
//! copied-span filter. So with the boundary on (hybrid) an image is by default
//! read by the local model, which describes it; the description is cleaned
//! like any local output and is all the frontier gets. The frontier receives
//! the image itself only when it is public by an explicit rule:
//!
//! - `images.to_frontier = "public"` and the image is a workspace file on a
//!   path that is neither sensitive nor protected; or
//! - the operator attached it marked public (`--image-public`,
//!   `/image --public`), which is recorded as the operator's decision.
//!
//! An image on a sensitive path never reaches the frontier, marked public or
//! not; one on a protected path (IP levels) is not shown or described at all.
//! Without a local model that reads images (`local.vision`), an image that
//! may not go to the frontier is refused with the reason, never sent anyway.
//! With the boundary off (pass-through) an image goes to the frontier when it
//! accepts images (`frontier.vision`) and is refused otherwise.

use std::path::PathBuf;

/// Where an image came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A workspace file, by its path relative to the workspace: read with
    /// `read_file`, or attached by the operator from inside the workspace.
    Workspace(PathBuf),
    /// Attached by the operator from outside the workspace (its file name).
    Attached(String),
}

impl Origin {
    /// `workspace:<path>` or `attached:<name>`, as audit events name it.
    pub fn label(&self) -> String {
        match self {
            Origin::Workspace(p) => format!("workspace:{}", p.display()),
            Origin::Attached(name) => format!("attached:{name}"),
        }
    }

    /// The path or file name, as the frontier and the operator see it.
    pub fn shown(&self) -> String {
        match self {
            Origin::Workspace(p) => p.display().to_string(),
            Origin::Attached(name) => name.clone(),
        }
    }
}

/// An image to route: where it came from, what the operator said about it,
/// the digest of the prepared bytes (what would be sent) and whether the
/// frontier model accepts images (`frontier.vision`).
#[derive(Debug, Clone, Copy)]
pub struct ImageRequest<'a> {
    pub origin: &'a Origin,
    /// Attached by the operator (rather than read by the frontier).
    pub attached: bool,
    /// The operator marked it public.
    pub operator_public: bool,
    pub sha256: &'a str,
    pub frontier_vision: bool,
}

/// Where an image goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The image itself joins the conversation; `rule` names why
    /// (`passthrough`, `images.to_frontier`, `operator_public`).
    Frontier { rule: &'static str },
    /// The local model describes it; the frontier gets the cleaned
    /// description. `rule` names why the image itself may not go.
    Describe { rule: &'static str },
    /// Nobody sees it. `rule` names the reason, `message` explains it.
    Refuse { rule: &'static str, message: String },
}

impl Route {
    /// `frontier`, `local` or `none`: where the image goes (audit events).
    pub fn destination(&self) -> &'static str {
        match self {
            Route::Frontier { .. } => "frontier",
            Route::Describe { .. } => "local",
            Route::Refuse { .. } => "none",
        }
    }

    /// The rule that decided.
    pub fn rule(&self) -> &'static str {
        match self {
            Route::Frontier { rule } | Route::Describe { rule } | Route::Refuse { rule, .. } => {
                rule
            }
        }
    }
}

/// `images.to_frontier`: which images the frontier may receive in hybrid mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToFrontier {
    /// Only images the operator attached marked public.
    #[default]
    Never,
    /// Also workspace images on paths that are neither sensitive nor protected.
    Public,
}

impl ToFrontier {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "never" => Some(Self::Never),
            "public" => Some(Self::Public),
            _ => None,
        }
    }
}

/// The settings and facts one decision depends on.
#[derive(Debug, Clone, Copy)]
pub struct Facts {
    /// The boundary is on (hybrid).
    pub boundary: bool,
    pub frontier_vision: bool,
    /// A local model that reads images is configured (`local.vision`).
    pub local_vision: bool,
    pub to_frontier: ToFrontier,
    /// Attached by the operator.
    pub attached: bool,
    pub operator_public: bool,
    /// A workspace image (not attached from outside the workspace).
    pub in_workspace: bool,
    /// Its path is sensitive (policy globs, or derived from sensitive data).
    pub sensitive: bool,
    /// Its path is protected source (IP levels).
    pub protected: bool,
}

const NO_FRONTIER_VISION: &str = "the frontier model does not accept images (frontier.vision is \
false)";
const NO_LOCAL_VISION: &str = "images that may not go to the frontier are described by the local \
model, and it does not read images (local.vision is false; `declass doctor --online` tests the model)";

/// The routing rule (see the module documentation).
pub fn route(f: &Facts) -> Route {
    let refuse = |rule, message: String| Route::Refuse { rule, message };
    if !f.boundary {
        return if f.frontier_vision {
            Route::Frontier {
                rule: "passthrough",
            }
        } else {
            refuse("no_frontier_vision", NO_FRONTIER_VISION.into())
        };
    }
    if f.protected {
        return refuse(
            "protected_path",
            "it is on a protected path (ip.interface_only or ip.sealed): images there are neither \
shown nor described"
                .into(),
        );
    }
    if f.sensitive {
        if f.operator_public {
            return refuse(
                "sensitive_path",
                "it is on a sensitive path, and sensitive-path images never go to the frontier, \
even marked public; attach it without --public to have the local model describe it"
                    .into(),
            );
        }
        return if f.local_vision {
            Route::Describe {
                rule: "sensitive_path",
            }
        } else {
            refuse(
                "no_local_vision",
                format!("it is on a sensitive path, and {NO_LOCAL_VISION}"),
            )
        };
    }
    let public = if f.operator_public {
        Some("operator_public")
    } else if f.in_workspace && f.to_frontier == ToFrontier::Public {
        Some("images.to_frontier")
    } else {
        None
    };
    match public {
        Some(rule) if f.frontier_vision => Route::Frontier { rule },
        Some(_) if f.local_vision => Route::Describe {
            rule: "no_frontier_vision",
        },
        Some(_) => refuse(
            "no_vision",
            format!("{NO_FRONTIER_VISION}, and {NO_LOCAL_VISION}"),
        ),
        None if f.local_vision => Route::Describe {
            rule: "images.to_frontier",
        },
        None => refuse(
            "no_local_vision",
            format!(
                "{NO_LOCAL_VISION}. The frontier gets an image itself only when it is public: {}",
                if f.attached {
                    "attach it with --image-public (or /image --public) if it holds nothing \
sensitive"
                } else {
                    "set images.to_frontier = \"public\" for workspace images on non-sensitive \
paths"
                }
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            boundary: true,
            frontier_vision: true,
            local_vision: true,
            to_frontier: ToFrontier::Never,
            attached: false,
            operator_public: false,
            in_workspace: true,
            sensitive: false,
            protected: false,
        }
    }

    /// Every combination of mode, path class, how the image arrived and
    /// both vision flags, against the rule written out case by case.
    #[test]
    fn rules_matrix() {
        let mut cases = 0;
        for boundary in [false, true] {
            for sensitive in [false, true] {
                for (attached, operator_public, in_workspace) in [
                    (false, false, true), // read_file
                    (true, false, true),  // attached from the workspace
                    (true, true, true),   // attached from the workspace, public
                    (true, false, false), // attached from elsewhere
                    (true, true, false),  // attached from elsewhere, public
                ] {
                    if sensitive && !in_workspace {
                        continue; // only workspace paths are classified
                    }
                    for to_frontier in [ToFrontier::Never, ToFrontier::Public] {
                        for frontier_vision in [false, true] {
                            for local_vision in [false, true] {
                                let f = Facts {
                                    boundary,
                                    frontier_vision,
                                    local_vision,
                                    to_frontier,
                                    attached,
                                    operator_public,
                                    in_workspace,
                                    sensitive,
                                    protected: false,
                                };
                                let expected = if !boundary {
                                    if frontier_vision { "frontier" } else { "none" }
                                } else if sensitive {
                                    if !operator_public && local_vision {
                                        "local"
                                    } else {
                                        "none"
                                    }
                                } else {
                                    let public = operator_public
                                        || (in_workspace && to_frontier == ToFrontier::Public);
                                    if public && frontier_vision {
                                        "frontier"
                                    } else if local_vision {
                                        "local"
                                    } else {
                                        "none"
                                    }
                                };
                                let got = route(&f);
                                assert_eq!(got.destination(), expected, "{f:?} -> {got:?}");
                                if let Route::Refuse { message, .. } = &got {
                                    assert!(!message.is_empty());
                                }
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(cases, 2 * (5 + 3) * 2 * 2 * 2);
    }

    #[test]
    fn sensitive_and_protected_images_never_reach_the_frontier() {
        for operator_public in [false, true] {
            for to_frontier in [ToFrontier::Never, ToFrontier::Public] {
                let f = Facts {
                    sensitive: true,
                    operator_public,
                    attached: operator_public,
                    to_frontier,
                    ..facts()
                };
                assert_ne!(route(&f).destination(), "frontier");
                let p = Facts {
                    protected: true,
                    operator_public,
                    attached: operator_public,
                    to_frontier,
                    ..facts()
                };
                assert_eq!(route(&p).rule(), "protected_path");
            }
        }
        // Marked public on a sensitive path: refused with the reason, not
        // quietly described instead.
        let f = Facts {
            sensitive: true,
            attached: true,
            operator_public: true,
            ..facts()
        };
        assert!(matches!(
            route(&f),
            Route::Refuse {
                rule: "sensitive_path",
                ..
            }
        ));
    }

    #[test]
    fn the_rule_that_decided_is_named() {
        assert_eq!(
            route(&Facts {
                operator_public: true,
                attached: true,
                ..facts()
            }),
            Route::Frontier {
                rule: "operator_public"
            }
        );
        assert_eq!(
            route(&Facts {
                to_frontier: ToFrontier::Public,
                ..facts()
            }),
            Route::Frontier {
                rule: "images.to_frontier"
            }
        );
        let refused = route(&Facts {
            local_vision: false,
            ..facts()
        });
        let Route::Refuse { rule, message } = refused else {
            panic!("{refused:?}")
        };
        assert_eq!(rule, "no_local_vision");
        assert!(message.contains("local.vision") && message.contains("images.to_frontier"));
        assert_eq!(
            Origin::Workspace("docs/ui.png".into()).label(),
            "workspace:docs/ui.png"
        );
        assert_eq!(ToFrontier::parse("public"), Some(ToFrontier::Public));
        assert_eq!(ToFrontier::parse("always"), None);
    }
}
