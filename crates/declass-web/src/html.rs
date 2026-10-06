// SPDX-License-Identifier: GPL-3.0-or-later
//! HTML to readable text.
//!
//! A small, forgiving tag scanner, not a full HTML parser: it keeps the text a
//! reader would see, marks headings, list items and table cells, keeps links
//! as `[text](absolute url)` so the model can follow them, and drops scripts,
//! styles, forms' hidden parts and the document head (except the title).
//! Malformed markup degrades to more text, never to an error.

use url::Url;

/// Elements whose content is never shown.
const SKIPPED: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "math", "head", "iframe", "object", "canvas",
    "select", "button",
];

/// Elements that start a new line.
const BLOCKS: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "header",
    "footer",
    "nav",
    "aside",
    "main",
    "ul",
    "ol",
    "dl",
    "dt",
    "dd",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "blockquote",
    "figure",
    "figcaption",
    "form",
    "fieldset",
    "details",
    "summary",
    "address",
    "hr",
    "br",
    "pre",
    "caption",
];

struct Tag<'a> {
    name: String,
    closing: bool,
    self_closing: bool,
    attrs: &'a str,
}

/// Converts an HTML document fetched from `base` to text.
pub fn to_text(html: &str, base: Option<&Url>) -> String {
    let mut out = Writer::default();
    let title = find_title(html);
    if let Some(t) = &title {
        out.text(&format!("Title: {t}"));
        out.newline(2);
    }
    let mut rest = html;
    // Open `<a>` elements: the resolved target, or `None` for a link not shown.
    let mut links: Vec<Option<String>> = Vec::new();
    let mut pre = 0usize;
    while let Some(lt) = rest.find('<') {
        out.chars(&rest[..lt], pre > 0);
        rest = &rest[lt..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |e| &after[e + 3..]);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            rest = rest.find('>').map_or("", |e| &rest[e + 1..]);
            continue;
        }
        let Some((tag, after)) = parse_tag(rest) else {
            // A lone `<` is text.
            out.chars("<", pre > 0);
            rest = &rest[1..];
            continue;
        };
        rest = after;
        let name = tag.name.as_str();
        if !tag.closing && SKIPPED.contains(&name) && !tag.self_closing {
            rest = skip_element(rest, name);
            continue;
        }
        match (name, tag.closing) {
            ("a", false) => {
                let href = attr(tag.attrs, "href").and_then(|h| link_target(&h, base));
                if href.is_some() {
                    out.text("[");
                }
                links.push(href);
            }
            ("a", true) => {
                if let Some(Some(href)) = links.pop() {
                    out.link_end(&href);
                }
            }
            ("img", _) => {
                if let Some(alt) = attr(tag.attrs, "alt").filter(|a| !a.trim().is_empty()) {
                    out.text(&format!("[image: {}]", alt.trim()));
                }
            }
            ("li", false) => {
                out.newline(1);
                out.text("- ");
            }
            ("td" | "th", false) => out.text(" | "),
            ("pre", false) => {
                out.newline(1);
                pre += 1;
            }
            ("pre", true) => {
                pre = pre.saturating_sub(1);
                out.newline(1);
            }
            (h, false) if is_heading(h) => {
                out.newline(2);
                let level = h[1..].parse::<usize>().unwrap_or(1);
                out.text(&format!("{} ", "#".repeat(level)));
            }
            (h, true) if is_heading(h) => out.newline(2),
            ("p", _) => out.newline(2),
            (b, _) if BLOCKS.contains(&b) => out.newline(1),
            _ => {}
        }
    }
    out.chars(rest, pre > 0);
    out.finish()
}

fn is_heading(name: &str) -> bool {
    name.len() == 2 && name.starts_with('h') && matches!(name.as_bytes()[1], b'1'..=b'6')
}

/// A link target worth showing: absolute http(s), not a same-page anchor or script.
fn link_target(href: &str, base: Option<&Url>) -> Option<String> {
    let href = decode_entities(href.trim());
    if href.is_empty() || href.starts_with('#') {
        return None;
    }
    let url = match base {
        Some(b) => b.join(&href).ok()?,
        None => Url::parse(&href).ok()?,
    };
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

fn find_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let end = lower[open_end..].find("</title")? + open_end;
    let t = collapse(&decode_entities(&html[open_end..end]));
    (!t.is_empty()).then_some(t)
}

/// Parses the tag at the start of `s` (which starts with `<`). Returns the tag
/// and the text after it, or `None` if this `<` does not start a tag.
fn parse_tag(s: &str) -> Option<(Tag<'_>, &str)> {
    let body = &s[1..];
    let (closing, body) = match body.strip_prefix('/') {
        Some(b) => (true, b),
        None => (false, body),
    };
    let name_len = body
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b':')
        .count();
    if name_len == 0 || !body.as_bytes()[0].is_ascii_alphabetic() {
        return None;
    }
    // The end of the tag: the first `>` outside a quoted attribute value.
    let bytes = body.as_bytes();
    let mut quote = None;
    let mut end = None;
    for (i, &b) in bytes.iter().enumerate().skip(name_len) {
        match (quote, b) {
            (Some(q), _) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'>') => {
                end = Some(i);
                break;
            }
            _ => {}
        }
    }
    let end = end?;
    let attrs = &body[name_len..end];
    Some((
        Tag {
            name: body[..name_len].to_ascii_lowercase(),
            closing,
            self_closing: attrs.trim_end().ends_with('/'),
            attrs,
        },
        &body[end + 1..],
    ))
}

/// The text after the end of element `name` whose start tag was just read
/// (nested elements of the same name are counted).
fn skip_element<'a>(s: &'a str, name: &str) -> &'a str {
    let lower = s.to_ascii_lowercase();
    let open = format!("<{name}");
    let close = format!("</{name}");
    let raw = matches!(name, "script" | "style");
    let mut depth = 1usize;
    let mut at = 0usize;
    loop {
        let next_close = lower[at..].find(&close).map(|i| i + at);
        let next_open = if raw {
            None
        } else {
            lower[at..].find(&open).map(|i| i + at)
        };
        match (next_open, next_close) {
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                at = o + open.len();
            }
            (_, Some(c)) => {
                depth -= 1;
                let end = lower[c..].find('>').map_or(lower.len(), |e| c + e + 1);
                if depth == 0 {
                    return &s[end..];
                }
                at = end;
            }
            (_, None) => return "",
        }
    }
}

/// The value of attribute `name` in a tag's attribute text.
fn attr(attrs: &str, name: &str) -> Option<String> {
    let bytes = attrs.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b'=' | b'/')
        {
            i += 1;
        }
        let key = attrs[start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && matches!(bytes[i], b'"' | b'\'') {
                let q = bytes[i];
                let vstart = i + 1;
                i = vstart;
                while i < bytes.len() && bytes[i] != q {
                    i += 1;
                }
                value = attrs[vstart..i.min(attrs.len())].to_owned();
                i += 1;
            } else {
                let vstart = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                value = attrs[vstart..i].to_owned();
            }
        }
        if key == name {
            return Some(value);
        }
        if i == start {
            i += 1;
        }
    }
    None
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Decodes character references: numeric ones and the common named ones.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let end = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '#'))
            .map(|i| i + 1);
        let (name, consumed) = match end {
            Some(e) if rest[e..].starts_with(';') => (&rest[1..e], e + 1),
            Some(e) => (&rest[1..e], e),
            None => (&rest[1..], rest.len()),
        };
        match entity(name) {
            Some(c) => {
                out.push(c);
                rest = &rest[consumed..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code).filter(|c| *c != '\0');
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "middot" => '·',
        "bull" => '•',
        "times" => '×',
        "rarr" => '→',
        "larr" => '←',
        _ => return None,
    })
}

/// Accumulates text with collapsed whitespace and bounded blank lines.
#[derive(Default)]
struct Writer {
    out: String,
    /// Newlines owed before the next text.
    pending: usize,
    /// A space is owed before the next text on this line.
    space: bool,
}

impl Writer {
    fn newline(&mut self, n: usize) {
        self.pending = self.pending.max(n);
        self.space = false;
    }

    fn flush(&mut self) {
        if self.out.is_empty() {
            self.pending = 0;
        }
        for _ in 0..self.pending {
            self.out.push('\n');
        }
        if self.pending == 0 && self.space && !self.out.ends_with([' ', '\n', '[']) {
            self.out.push(' ');
        }
        self.pending = 0;
        self.space = false;
    }

    /// Text content from the document.
    fn chars(&mut self, raw: &str, preformatted: bool) {
        if raw.is_empty() {
            return;
        }
        let text = decode_entities(raw);
        if preformatted {
            self.flush();
            self.out.push_str(&text);
            return;
        }
        let leading = text.starts_with(char::is_whitespace);
        let trailing = text.ends_with(char::is_whitespace);
        let words = collapse(&text);
        if words.is_empty() {
            self.space |= leading || trailing;
            return;
        }
        self.space |= leading;
        self.flush();
        self.out.push_str(&words);
        self.space = trailing;
    }

    /// Text Declass adds (markers), placed like content.
    fn text(&mut self, s: &str) {
        self.flush();
        self.out.push_str(s);
    }

    fn link_end(&mut self, href: &str) {
        if self.out.ends_with('[') {
            // A link without text (an icon): drop the bracket, show nothing.
            self.out.pop();
            return;
        }
        self.out.push_str(&format!("]({href})"));
    }

    fn finish(self) -> String {
        let mut lines: Vec<&str> = Vec::new();
        let mut blank = 0;
        for l in self.out.lines().map(str::trim_end) {
            if l.trim().is_empty() {
                blank += 1;
                if blank > 1 || lines.is_empty() {
                    continue;
                }
            } else {
                blank = 0;
            }
            lines.push(l);
        }
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        let mut s = lines.join("\n");
        s.push('\n');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_text_keeps_structure_and_links_and_drops_scripts() {
        let html = r##"<!DOCTYPE html><html><head><title>Rust &amp; You</title>
<style>body { color: red }</style><script>var x = "<p>not text</p>";</script></head>
<body><nav><a href="/">Home</a> <a href="#top">Top</a></nav>
<h1>Getting <em>started</em></h1>
<p>Install with <code>rustup</code>.&nbsp;See the <a href="docs/book.html">book</a>&#46;</p>
<ul><li>One</li><li>Two &lt;three&gt;</li></ul>
<table><tr><th>Name</th><th>Value</th></tr><tr><td>a</td><td>1</td></tr></table>
<pre>fn main() {
    println!("hi");
}</pre>
<!-- a comment <p>hidden</p> -->
<img src="x.png" alt="Logo"><a href="javascript:alert(1)">click</a>
</body></html>"##;
        let base = Url::parse("https://example.org/learn/").unwrap();
        let text = to_text(html, Some(&base));
        assert!(text.starts_with("Title: Rust & You\n"), "{text}");
        assert!(text.contains("[Home](https://example.org/)"), "{text}");
        assert!(text.contains("Top") && !text.contains("#top"), "{text}");
        assert!(text.contains("# Getting started"), "{text}");
        assert!(
            text.contains("See the [book](https://example.org/learn/docs/book.html)."),
            "{text}"
        );
        assert!(text.contains("- One\n- Two <three>"), "{text}");
        assert!(text.contains("| Name | Value"), "{text}");
        assert!(
            text.contains("fn main() {\n    println!(\"hi\");\n}"),
            "{text}"
        );
        assert!(text.contains("[image: Logo]"), "{text}");
        assert!(
            text.contains("click") && !text.contains("javascript"),
            "{text}"
        );
        for gone in ["color: red", "not text", "hidden", "DOCTYPE"] {
            assert!(!text.contains(gone), "{gone} in {text}");
        }
    }

    #[test]
    fn malformed_markup_degrades_to_text() {
        let text = to_text("a < b and <b>bold</b> <p unclosed", None);
        assert!(text.contains("a < b and bold"), "{text}");
        let text = to_text("<script>never closed", None);
        assert_eq!(text.trim(), "");
        assert_eq!(decode_entities("&#x41;&#66;&bogus;&amp"), "AB&bogus;&");
    }
}
