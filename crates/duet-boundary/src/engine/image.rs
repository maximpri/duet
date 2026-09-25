// SPDX-License-Identifier: GPL-3.0-or-later
//! Images in the engine: the hybrid routing rule applied to a path's class,
//! the digests of the images the frontier may receive, and the local model's
//! description of an image the frontier may not see.

use super::{Engine, State};
use crate::images::{Facts, ImageRequest, Origin, Route};
use crate::model::Image;
use crate::view::ViewClass;

impl Engine {
    /// The hybrid rule (see [`crate::images`]); an image routed to the
    /// frontier is recorded by digest, and no other may be sent.
    pub(super) fn image_route(&self, image: &ImageRequest<'_>) -> Route {
        let (in_workspace, sensitive, protected) = match image.origin {
            Origin::Workspace(p) => (
                true,
                self.is_sensitive(p),
                self.policy.ip_level(p).is_some(),
            ),
            Origin::Attached(_) => (false, false, false),
        };
        let route = crate::images::route(&Facts {
            boundary: true,
            frontier_vision: image.frontier_vision,
            local_vision: self.policy.local_vision && self.local.is_some(),
            to_frontier: self.policy.images_to_frontier,
            attached: image.attached,
            operator_public: image.operator_public,
            in_workspace,
            sensitive,
            protected,
        });
        if let Route::Frontier { .. } = route {
            self.allow_image(image.sha256);
        }
        route
    }

    /// Local-model output about content Duet cannot read as text (an
    /// image): sanitized as sensitive text with every name-like phrase
    /// treated as a person (nothing tells what the model took from the
    /// content), then copied spans removed, strictly.
    pub(super) fn clean_unseen(&self, st: &mut State, text: &str, origin: &str) -> String {
        let s = self.sanitize_read(st, text, origin, true, None);
        let s = st.overlap.redact(&s).0;
        st.overlap.redact_strict(&s).0
    }

    /// An image the frontier may not see: kept under a handle and described
    /// by the local model; the frontier gets the cleaned description and the
    /// handle for `ask_local`.
    pub(super) fn image_view(&self, origin: &str, bytes: &[u8]) -> String {
        self.set_class(ViewClass::HandleSummary);
        let image = match Image::from_encoded(bytes.to_vec()) {
            Ok(img) => img,
            Err(e) => return format!("[image {origin} withheld: {e}]"),
        };
        let local = match &self.local {
            Some(l) if self.policy.local_vision => l,
            _ => {
                return format!(
                    "[image {origin} withheld: the local model does not read images (local.vision is false)]"
                );
            }
        };
        let label = format!("image {origin}");
        let handle = match self.lock().handles.put(bytes, &label) {
            Ok(h) => h,
            Err(e) => return format!("[image {origin} withheld: could not store it locally: {e}]"),
        };
        let described = Self::block_on(local.describe_image(&label, &image));
        let mut st = self.lock();
        let mut out = format!(
            "{} ({label}, {}): the image stays on this machine; the local model describes it (text \
in the image is paraphrased and sensitive values are withheld). Ask about details with \
ask_local(handle=\"{}\", questions=[...]).\n",
            handle.id,
            image.describe(),
            handle.id
        );
        match described {
            Ok(d) => {
                out.push_str(&format!(
                    "Description by the local model: {}\n",
                    self.clean_unseen(&mut st, &d.summary, &label)
                ));
                for f in &d.facts {
                    out.push_str(&format!("- {}\n", self.clean_unseen(&mut st, f, &label)));
                }
            }
            Err(e) => out.push_str(&format!(
                "[local description unavailable: {}]\n",
                e.message.chars().take(160).collect::<String>()
            )),
        }
        out
    }

    /// Records that the image `sha256` may be sent to the frontier.
    fn allow_image(&self, sha256: &str) {
        let mut st = self.lock();
        if st.frontier_images.insert(sha256.to_owned()) {
            let _ = duet_fs::private::write_private(
                &self.images_file,
                &serde_json::to_vec(&st.frontier_images).unwrap_or_default(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::ToFrontier;
    use crate::model::{Item, Request, prepare_image};
    use crate::policy::Policy;
    use crate::testing::canary::Canaries;
    use crate::view::{Presenter, Source};
    use duet_provider::image::solid_png;
    use serde_json::{Map, json};

    const KEY: &str = "sk_live_Qm8vT2xW9pL4nR7kZ3cY6bH1";
    const EMAIL: &str = "amelia.velanwick42@mailbox-311.net";
    const NAME: &str = "Amelia Velanwick";
    const CARD: &str = "4539 1488 0343 6467";

    fn policy(to_frontier: ToFrontier, local_vision: bool) -> Policy {
        Policy {
            sensitive_globs: vec!["data/**".into(), "*.env".into()],
            sealed: vec!["src/secret/**".into()],
            detect_secrets: true,
            detect_pii: true,
            detect_entropy: true,
            local_vision,
            images_to_frontier: to_frontier,
            ..Policy::default()
        }
    }

    fn image(rgb: [u8; 3]) -> Image {
        prepare_image(&solid_png(8, 8, rgb), 64).unwrap()
    }

    fn request<'a>(origin: &'a Origin, img: &'a Image, public: bool) -> ImageRequest<'a> {
        ImageRequest {
            origin,
            attached: public,
            operator_public: public,
            sha256: &img.sha256,
            frontier_vision: true,
        }
    }

    #[test]
    fn only_images_routed_to_the_frontier_may_be_sent() {
        let d = tempfile::tempdir().unwrap();
        let (local, _) = crate::testing::scripted_local(vec![]);
        let e = Engine::open(d.path(), policy(ToFrontier::Public, true), Some(local)).unwrap();
        let public = image([1, 1, 1]);
        let sensitive = image([2, 2, 2]);
        let sealed = image([3, 3, 3]);
        let docs = Origin::Workspace("docs/ui.png".into());
        let data = Origin::Workspace("data/scan.png".into());
        let secret = Origin::Workspace("src/secret/logo.png".into());
        assert_eq!(
            e.route_image(&request(&docs, &public, false)),
            Route::Frontier {
                rule: "images.to_frontier"
            }
        );
        // Marked public or not, a sensitive-path image stays here.
        assert_eq!(
            e.route_image(&request(&data, &sensitive, false)),
            Route::Describe {
                rule: "sensitive_path"
            }
        );
        assert_eq!(
            e.route_image(&request(&data, &sensitive, true)).rule(),
            "sensitive_path"
        );
        assert_eq!(
            e.route_image(&request(&secret, &sealed, true)).rule(),
            "protected_path"
        );
        // A file a sensitive command wrote is sensitive from then on.
        e.mark_sensitive(d.path(), &["docs/derived.png".into()]);
        let derived = Origin::Workspace("docs/derived.png".into());
        assert_eq!(
            e.route_image(&request(&derived, &sensitive, false))
                .destination(),
            "local"
        );

        let (filter, check) = e.outbound();
        let mut req = Request {
            items: vec![
                Item::User {
                    text: "look".into(),
                },
                Item::Images {
                    call_id: None,
                    images: vec![public.clone(), sensitive.clone(), sealed.clone()],
                },
            ],
            ..Request::default()
        };
        let notes = filter.apply(&mut req);
        assert_eq!(
            notes,
            vec!["withheld 2 image(s) not approved for the frontier"]
        );
        let Item::Images { images, .. } = &req.items[1] else {
            panic!()
        };
        assert_eq!(images, &vec![public.clone()]);
        assert!(
            check
                .check_images(std::slice::from_ref(&public.sha256))
                .is_ok()
        );
        let refused = check
            .check_images(&[public.sha256.clone(), sensitive.sha256.clone()])
            .unwrap_err();
        assert!(refused.contains("not approved"), "{refused}");
        // The approval outlives the process (a resumed run re-sends it).
        drop((filter, check, e));
        let e = Engine::open(d.path(), policy(ToFrontier::Public, true), None).unwrap();
        let (_, check) = e.outbound();
        assert!(
            check
                .check_images(std::slice::from_ref(&public.sha256))
                .is_ok()
        );
        assert!(
            check
                .check_images(std::slice::from_ref(&sensitive.sha256))
                .is_err()
        );
    }

    #[test]
    fn without_local_vision_an_image_is_refused_not_described() {
        let d = tempfile::tempdir().unwrap();
        let (local, received) = crate::testing::scripted_local(vec![]);
        let e = Engine::open(d.path(), policy(ToFrontier::Never, false), Some(local)).unwrap();
        let img = image([4, 4, 4]);
        let docs = Origin::Workspace("docs/ui.png".into());
        let route = e.route_image(&request(&docs, &img, false));
        assert_eq!(route.rule(), "no_local_vision");
        // Even asked directly, the engine does not show or describe it.
        let shown = e.present(
            &Source::Image {
                origin: "docs/ui.png".into(),
            },
            img.data(),
        );
        assert!(shown.contains("withheld") && shown.contains("local.vision"));
        assert!(received.bodies().is_empty());
        let (_, check) = e.outbound();
        assert!(
            check
                .check_images(std::slice::from_ref(&img.sha256))
                .is_err()
        );
    }

    fn describes(summary: &str, facts: &[&str]) -> String {
        json!({"summary": summary, "facts": facts}).to_string()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_description_is_cleaned_strictly_and_questions_see_the_image_again() {
        let leaky = format!(
            "A billing screenshot for {NAME} ({EMAIL}); the card field shows {CARD} and the \
             settings pane shows the key {KEY}."
        );
        let (local, received) = crate::testing::scripted_local(vec![
            describes(
                &leaky,
                &[&format!("Contact: {EMAIL}"), "A red Save button."],
            ),
            json!({"answer": format!("The key is {KEY}."), "evidence_lines": [3],
                   "unanswerable": false})
            .to_string(),
        ]);
        let d = tempfile::tempdir().unwrap();
        let e = Engine::open(d.path(), policy(ToFrontier::Never, true), Some(local)).unwrap();
        let img = image([200, 10, 10]);
        let shown = e.present(
            &Source::Image {
                origin: "data/billing.png".into(),
            },
            img.data(),
        );
        assert_eq!(e.take_view_class(), Some(ViewClass::HandleSummary));
        let planted = Canaries::new([KEY, EMAIL, NAME, "Velanwick", CARD]);
        let found = planted.find(&shown);
        assert!(found.is_empty(), "{found:?} crossed: {shown}");
        assert!(shown.contains("red Save button"), "{shown}");
        assert!(
            shown.starts_with("h1 (image data/billing.png, 8x8 png"),
            "{shown}"
        );
        // The local model got the image itself, before the instruction.
        let body = &received.bodies()[0];
        let content = &body["messages"][1]["content"];
        assert_eq!(content[0]["image_url"]["url"], img.data_url());
        assert!(
            content[1]["text"]
                .as_str()
                .unwrap()
                .contains("data/billing.png")
        );

        let mut args = Map::new();
        args.insert("handle".into(), json!("h1"));
        args.insert("question".into(), json!("Which key is shown?"));
        let answer = e.call_tool("ask_local", &args).unwrap().unwrap();
        assert!(planted.find(&answer).is_empty(), "{answer}");
        let again = &received.bodies()[1]["messages"][1]["content"][0];
        assert_eq!(again["image_url"]["url"], img.data_url());
        // The values the description revealed are known from now on: the
        // outbound filter replaces them wherever they appear.
        let (filter, check) = e.outbound();
        let mut req = Request {
            items: vec![Item::User {
                text: format!("mail {EMAIL} about {KEY}"),
            }],
            ..Request::default()
        };
        filter.apply(&mut req);
        let body = serde_json::to_value(&req).unwrap().to_string();
        assert!(!body.contains(EMAIL) && !body.contains(KEY), "{body}");
        assert!(check.check(&serde_json::to_value(&req).unwrap()).is_ok());
    }
}
