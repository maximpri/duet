// SPDX-License-Identifier: GPL-3.0-or-later
//! CycloneDX 1.5 JSON from `cargo metadata --format-version 1` output and the
//! lock file.
//!
//! Components are the packages the root package needs to build and run:
//! normal and build dependencies, transitively, for the platform the metadata
//! was filtered to; dev-dependencies are left out. Each component carries its
//! package URL (`pkg:cargo/<name>@<version>`), its license expression as
//! declared in its manifest, the SHA-256 of its registry archive from
//! `Cargo.lock`, and its repository. Local paths never appear. The serial
//! number is derived from the lock file and the root package, so the same
//! inputs give the same document.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

pub const SPEC_VERSION: &str = "1.5";

/// Inputs besides the metadata and lock file.
pub struct Options<'a> {
    /// Name of the workspace package the SBOM describes (e.g. `declass-cli`).
    pub root: &'a str,
    /// RFC 3339 creation time.
    pub timestamp: &'a str,
    /// The tool's own version, recorded in the metadata.
    pub tool_version: &'a str,
}

fn purl(name: &str, version: &str) -> String {
    format!("pkg:cargo/{name}@{version}")
}

/// A manifest license as an SPDX expression (`MIT/Apache-2.0` is the old
/// spelling of `MIT OR Apache-2.0`).
pub fn license_expression(license: &str) -> String {
    license
        .split('/')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// `name@version` → SHA-256 of the registry archive, from `Cargo.lock`.
fn checksums(lock: &str) -> Result<HashMap<String, String>, String> {
    let table: toml::Table = toml::from_str(lock).map_err(|e| format!("Cargo.lock: {e}"))?;
    let mut out = HashMap::new();
    for p in table
        .get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        let field = |k: &str| p.get(k).and_then(toml::Value::as_str);
        if let (Some(n), Some(v), Some(c)) = (field("name"), field("version"), field("checksum")) {
            out.insert(format!("{n}@{v}"), c.to_owned());
        }
    }
    Ok(out)
}

/// A UUID (version 4 layout) derived from `seed`.
fn serial(seed: &[u8]) -> String {
    let mut b: [u8; 16] = Sha256::digest(seed)[..16].try_into().unwrap_or([0; 16]);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

fn component(pkg: &Value, kind: &str, sums: &HashMap<String, String>) -> Value {
    let name = pkg["name"].as_str().unwrap_or_default();
    let version = pkg["version"].as_str().unwrap_or_default();
    let reference = purl(name, version);
    let mut c = json!({
        "type": kind,
        "bom-ref": reference,
        "name": name,
        "version": version,
        "purl": reference,
    });
    if let Some(d) = pkg["description"].as_str() {
        c["description"] = d.trim().into();
    }
    if let Some(l) = pkg["license"].as_str() {
        c["licenses"] = json!([{"expression": license_expression(l)}]);
    }
    if let Some(sum) = sums.get(&format!("{name}@{version}")) {
        c["hashes"] = json!([{"alg": "SHA-256", "content": sum}]);
    }
    if let Some(r) = pkg["repository"].as_str() {
        c["externalReferences"] = json!([{"type": "vcs", "url": r}]);
    }
    c
}

/// The SBOM for `opts.root`.
pub fn cyclonedx(metadata: &Value, lock: &str, opts: &Options<'_>) -> Result<Value, String> {
    let packages: HashMap<&str, &Value> = metadata["packages"]
        .as_array()
        .ok_or("metadata has no packages")?
        .iter()
        .filter_map(|p| Some((p["id"].as_str()?, p)))
        .collect();
    let members: BTreeSet<&str> = metadata["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let root_id = members
        .iter()
        .copied()
        .find(|id| packages.get(id).and_then(|p| p["name"].as_str()) == Some(opts.root))
        .ok_or_else(|| format!("{} is not a workspace package", opts.root))?;
    // Package id → ids of its normal and build dependencies.
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for node in metadata["resolve"]["nodes"]
        .as_array()
        .ok_or("metadata has no resolve graph (run without --no-deps)")?
    {
        let Some(id) = node["id"].as_str() else {
            continue;
        };
        let deps = node["deps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| {
                d["dep_kinds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|k| k["kind"].as_str() != Some("dev"))
            })
            .filter_map(|d| d["pkg"].as_str())
            .collect();
        edges.insert(id, deps);
    }
    let mut needed: BTreeSet<&str> = BTreeSet::from([root_id]);
    let mut queue = VecDeque::from([root_id]);
    while let Some(id) = queue.pop_front() {
        for dep in edges.get(id).into_iter().flatten() {
            if needed.insert(dep) {
                queue.push_back(dep);
            }
        }
    }
    let sums = checksums(lock)?;
    let reference = |id: &str| -> Option<String> {
        let p = packages.get(id)?;
        Some(purl(p["name"].as_str()?, p["version"].as_str()?))
    };
    let mut components = BTreeMap::new();
    for id in needed.iter().filter(|id| **id != root_id) {
        let pkg = packages
            .get(id)
            .ok_or_else(|| format!("unknown package {id}"))?;
        let c = component(pkg, "library", &sums);
        components.insert(c["bom-ref"].as_str().unwrap_or_default().to_owned(), c);
    }
    let mut dependencies = BTreeMap::new();
    for id in &needed {
        let Some(r) = reference(id) else { continue };
        let on: BTreeSet<String> = edges
            .get(id)
            .into_iter()
            .flatten()
            .filter_map(|d| reference(d))
            .collect();
        dependencies.insert(r.clone(), json!({"ref": r, "dependsOn": on}));
    }
    let root = component(packages[root_id], "application", &sums);
    let seed = format!("{}\n{}\n{lock}", root["bom-ref"], opts.timestamp);
    Ok(json!({
        "bomFormat": "CycloneDX",
        "specVersion": SPEC_VERSION,
        "serialNumber": serial(seed.as_bytes()),
        "version": 1,
        "metadata": {
            "timestamp": opts.timestamp,
            "tools": {"components": [{
                "type": "application",
                "name": "declass-sbom",
                "version": opts.tool_version,
            }]},
            "component": root,
        },
        "components": components.into_values().collect::<Vec<_>>(),
        "dependencies": dependencies.into_values().collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REG: &str = "registry+https://github.com/rust-lang/crates.io-index";

    fn pkg(id: &str, name: &str, license: Option<&str>) -> Value {
        json!({"id": id, "name": name, "version": "1.0.0", "license": license,
            "repository": format!("https://example.test/{name}"), "description": "d",
            "manifest_path": format!("/home/someone/src/{name}/Cargo.toml")})
    }

    fn dep(id: &str, kinds: &[Option<&str>]) -> Value {
        let kinds: Vec<Value> = kinds
            .iter()
            .map(|k| json!({"kind": k, "target": null}))
            .collect();
        json!({"name": "x", "pkg": id, "dep_kinds": kinds})
    }

    fn metadata() -> Value {
        let app = "path+file:///home/someone/src/app#0.1.0";
        let (a, b, t, bs) = (
            format!("{REG}#liba@1.0.0"),
            format!("{REG}#libb@1.0.0"),
            format!("{REG}#testonly@1.0.0"),
            format!("{REG}#buildhelper@1.0.0"),
        );
        let mut app_pkg = pkg(app, "app", Some("GPL-3.0-or-later"));
        app_pkg["version"] = "0.1.0".into();
        json!({
            "workspace_members": [app],
            "packages": [app_pkg, pkg(&a, "liba", Some("MIT/Apache-2.0")),
                pkg(&b, "libb", Some("MIT")), pkg(&t, "testonly", Some("MIT")),
                pkg(&bs, "buildhelper", None)],
            "resolve": {"nodes": [
                {"id": app, "deps": [dep(&a, &[None]), dep(&t, &[Some("dev")]),
                    dep(&bs, &[Some("build")])]},
                {"id": a, "deps": [dep(&b, &[Some("dev"), None])]},
                {"id": b, "deps": []},
                {"id": t, "deps": [dep(&b, &[None])]},
                {"id": bs, "deps": []},
            ]}
        })
    }

    const LOCK: &str = r#"
version = 4
[[package]]
name = "liba"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aa11"
[[package]]
name = "app"
version = "0.1.0"
"#;

    fn opts() -> Options<'static> {
        Options {
            root: "app",
            timestamp: "2026-09-24T00:00:00Z",
            tool_version: "0.1.0",
        }
    }

    #[test]
    fn components_are_the_runtime_and_build_closure_with_licenses_and_hashes() {
        let bom = cyclonedx(&metadata(), LOCK, &opts()).unwrap();
        assert_eq!(bom["bomFormat"], "CycloneDX");
        assert_eq!(bom["specVersion"], "1.5");
        assert_eq!(bom["metadata"]["component"]["name"], "app");
        assert_eq!(
            bom["metadata"]["component"]["licenses"][0]["expression"],
            "GPL-3.0-or-later"
        );
        let names: Vec<&str> = bom["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        // Sorted by purl; the dev-only package is absent, the dependency it
        // shares with a runtime package (a dev and normal edge) is present.
        assert_eq!(names, ["buildhelper", "liba", "libb"]);
        let liba = &bom["components"][1];
        assert_eq!(liba["purl"], "pkg:cargo/liba@1.0.0");
        assert_eq!(liba["licenses"][0]["expression"], "MIT OR Apache-2.0");
        assert_eq!(liba["hashes"][0]["content"], "aa11");
        assert!(bom["components"][0].get("licenses").is_none());
        let deps = bom["dependencies"].as_array().unwrap();
        let app = deps
            .iter()
            .find(|d| d["ref"] == "pkg:cargo/app@0.1.0")
            .unwrap();
        assert_eq!(
            app["dependsOn"],
            json!(["pkg:cargo/buildhelper@1.0.0", "pkg:cargo/liba@1.0.0"])
        );
        // No local path leaks into the document.
        assert!(!bom.to_string().contains("/home/someone"), "{bom}");
    }

    #[test]
    fn the_document_is_reproducible_and_the_serial_is_a_uuid() {
        let a = cyclonedx(&metadata(), LOCK, &opts()).unwrap();
        let b = cyclonedx(&metadata(), LOCK, &opts()).unwrap();
        assert_eq!(a, b);
        let serial = a["serialNumber"].as_str().unwrap();
        let uuid = serial.strip_prefix("urn:uuid:").unwrap();
        let groups: Vec<usize> = uuid.split('-').map(str::len).collect();
        assert_eq!(groups, [8, 4, 4, 4, 12]);
        assert_eq!(&uuid[14..15], "4");
        let other = cyclonedx(&metadata(), &format!("{LOCK}\n# changed"), &opts()).unwrap();
        assert_ne!(other["serialNumber"], a["serialNumber"]);
    }

    #[test]
    fn an_unknown_root_is_an_error() {
        let mut o = opts();
        o.root = "nope";
        assert!(cyclonedx(&metadata(), LOCK, &o).is_err());
    }
}
