//! Web tools for the model: search through SearXNG, fetch pages as plain text.

use regex::Regex;
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

const UA: &str = "CodeCanvas";

#[derive(Clone)]
pub struct WebTools { pub searxng: String, pub max_results: usize }

impl WebTools {
    pub fn search(&self, query: &str) -> String {
        let base = self.searxng.trim_end_matches('/');
        let r = ureq::get(&format!("{base}/search")).query("q", query).query("format", "json").set("accept", "application/json").set("user-agent", UA).timeout(Duration::from_secs(15)).call();
        let resp = match r {
            Ok(r) => r,
            Err(ureq::Error::Status(c @ (403 | 429), _)) => return format!("SEARCH FAILED: SearXNG answered HTTP {c}. JSON output is probably disabled: add \"json\" under search.formats in its settings.yml."),
            Err(ureq::Error::Status(c, _)) => return format!("SEARCH FAILED: SearXNG answered HTTP {c}."),
            Err(e) => return format!("SEARCH FAILED: couldn't reach SearXNG at {base} ({e})."),
        };
        let Ok(j) = resp.into_json::<Value>() else { return "SEARCH FAILED: SearXNG did not return JSON.".into() };
        let results: Vec<String> = j["results"].as_array().into_iter().flatten().take(self.max_results).enumerate().map(|(i, r)| {
            let snippet: String = clean(r["content"].as_str().unwrap_or("")).chars().take(300).collect();
            format!("{}. {}\n   {}\n   {snippet}", i + 1, clean(r["title"].as_str().unwrap_or("(untitled)")), r["url"].as_str().unwrap_or(""))
        }).collect();
        if results.is_empty() { format!("No results for \"{query}\".") } else { results.join("\n") }
    }

    pub fn fetch_page(&self, url: &str) -> String {
        if !(url.starts_with("http://") || url.starts_with("https://")) { return "FETCH FAILED: only http(s) URLs can be fetched.".into(); }
        let resp = match ureq::get(url).set("user-agent", UA).timeout(Duration::from_secs(20)).call() {
            Ok(r) => r,
            Err(e) => return format!("FETCH FAILED: {e}"),
        };
        let ty = resp.content_type().to_string();
        if !["text", "json", "xml", "javascript"].iter().any(|k| ty.contains(k)) { return format!("FETCH FAILED: {ty} content can't be read as text."); }
        let mut raw = Vec::new();
        if resp.into_reader().take(4_000_000).read_to_end(&mut raw).is_err() { return "FETCH FAILED: couldn't read the page.".into(); }
        let raw = String::from_utf8_lossy(&raw);
        let text = if ty.contains("html") { html_to_text(&raw) } else { raw.into_owned() };
        if text.chars().count() > 12000 { format!("{}\n…(truncated)", text.chars().take(12000).collect::<String>()) } else { text }
    }
}

fn decode(s: &str) -> String {
    s.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'").replace("&amp;", "&")
}

fn clean(s: &str) -> String {
    let tags = Regex::new(r"<[^>]+>").unwrap();
    decode(&tags.replace_all(s, "")).split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn html_to_text(html: &str) -> String {
    let mut s = html.to_string();
    for tag in ["script", "style", "noscript", "svg", "nav", "footer", "header", "form", "iframe"] {
        s = Regex::new(&format!(r"(?is)<{tag}\b.*?</{tag}>")).unwrap().replace_all(&s, "").into_owned();
    }
    if let Some(m) = Regex::new(r"(?is)<(?:main|article)\b.*</(?:main|article)>").unwrap().find(&s) {
        if m.as_str().len() > 500 || !s.contains("<p") { s = m.as_str().to_string(); }
    }
    let tags = Regex::new(r"<[^>]+>").unwrap();
    s = Regex::new(r"(?is)<pre[^>]*>(.*?)</pre>").unwrap().replace_all(&s, |c: &regex::Captures| format!("\n```\n{}\n```\n", tags.replace_all(&c[1], ""))).into_owned();
    s = Regex::new(r"(?i)<(br|hr)\s*/?>").unwrap().replace_all(&s, "\n").into_owned();
    s = Regex::new(r"(?i)</(p|div|li|h[1-6]|tr|section|article|blockquote|table)>").unwrap().replace_all(&s, "\n").into_owned();
    s = Regex::new(r"(?i)<li[^>]*>").unwrap().replace_all(&s, "- ").into_owned();
    s = Regex::new(r"(?i)<h([1-6])[^>]*>").unwrap().replace_all(&s, |c: &regex::Captures| format!("\n{} ", "#".repeat(c[1].parse().unwrap_or(1)))).into_owned();
    s = tags.replace_all(&s, "").into_owned();
    let s = decode(&s);
    let s = Regex::new(r"[ \t]+\n").unwrap().replace_all(&s, "\n").into_owned();
    Regex::new(r"\n{3,}").unwrap().replace_all(&s, "\n\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_keeps_content_and_code() {
        let t = html_to_text("<html><style>x{}</style><nav>menu</nav><main><h1>Foo</h1><p>New: <code>w()</code> &amp; more.</p><pre>foo(1)</pre></main></html>");
        assert_eq!(t, "# Foo\nNew: w() & more.\n\n```\nfoo(1)\n```");
    }
}
