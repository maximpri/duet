//! `.env` files: `KEY=value` lines, optional `export `, optional quotes.

use std::collections::BTreeMap;
use std::path::Path;

pub fn parse(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        let v = if v.len() >= 2 && (v.starts_with('"') && v.ends_with('"') || v.starts_with('\'') && v.ends_with('\''))
        {
            &v[1..v.len() - 1]
        } else {
            v
        };
        out.insert(k.trim().to_owned(), v.to_owned());
    }
    out
}

/// Settings from `<root>/.env`, overridden by the process environment for the
/// keys listed in `keys`. A missing file yields an empty map.
pub fn load(root: &Path, keys: &[&str]) -> BTreeMap<String, String> {
    let mut map = std::fs::read_to_string(root.join(".env")).map(|t| parse(&t)).unwrap_or_default();
    for key in keys {
        if let Ok(v) = std::env::var(key) {
            map.insert((*key).to_owned(), v);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dotenv_lines() {
        let m = parse("# c\nA=1\nexport B=\"two words\"\nC='x=y'\nnot a pair\n D = 4 \n");
        assert_eq!(m.get("A").map(String::as_str), Some("1"));
        assert_eq!(m.get("B").map(String::as_str), Some("two words"));
        assert_eq!(m.get("C").map(String::as_str), Some("x=y"));
        assert_eq!(m.get("D").map(String::as_str), Some("4"));
        assert_eq!(m.len(), 4);
    }

    #[test]
    fn missing_file_is_empty() {
        assert!(load(Path::new("/nonexistent-parcelflow-root"), &[]).is_empty());
    }
}
