//! Prompts, the plain-text tool loop (read files / search / fetch), drafted snippets, and Explain.

use crate::llm::{Cancel, LlmConfig, Message, stream_chat};
use crate::model::Project;
use crate::web::WebTools;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

#[derive(Clone, Debug, PartialEq)]
pub struct ContextItem { pub is_file: bool, pub label: String, pub path: Option<String>, pub start_line: Option<usize>, pub text: String }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft { pub title: String, pub file: Option<String>, pub insert_at: Option<usize>, pub replace: Option<(usize, usize)>, pub code: String, pub lang: Option<String> }

#[derive(Clone, Debug, PartialEq)]
pub struct Segment { pub start: usize, pub end: usize, pub title: String, pub explanation: String }

const SNIPPET_PROTOCOL: &str = r#"## Drafting snippets
When you propose code, emit it as a snippet block so it appears on the user's canvas as a draggable card:

```snippet file="relative/path.ext" insert="LINE" title="Short title"
...code...
```

- Use insert="N" to insert the code before line N of that file, or replace="A-B" to replace lines A..B (inclusive, 1-based).
- Omit file/insert/replace when the code is not tied to a location.
- Keep each snippet focused. Preserve the file's indentation exactly.

## Reading project files
You have read access to the whole project. To read a file, output exactly <read path="relative/path"/> on its own line and stop.
The file will be returned to you and you can continue. Only request files you actually need."#;

const WEB_PROTOCOL: &str = r#"## Searching the web
Output exactly <search query="your search terms"/> on its own line and stop; results (title, URL, snippet) are returned to you.
To read a page, output <fetch url="https://..."/> on its own line and stop. Mention the URLs you relied on."#;

fn numbered(text: &str, start: usize) -> String {
    text.split('\n').enumerate().map(|(i, l)| format!("{:>4}| {l}", i + start)).collect::<Vec<_>>().join("\n")
}

fn render_context(items: &[ContextItem]) -> String {
    if items.is_empty() { return String::new(); }
    let parts: Vec<String> = items.iter().map(|it| {
        let kind = if it.is_file { "File" } else { "Snippet" };
        let start = it.start_line.unwrap_or(1);
        let place = match (&it.path, it.start_line) {
            (Some(p), Some(s)) => format!(" — {p} (lines {s}-{})", s + it.text.split('\n').count() - 1),
            (Some(p), None) if *p != it.label => format!(" — {p}"),
            _ => String::new(),
        };
        format!("### {kind}: {}{place}\n```\n{}\n```", it.label, numbered(&it.text, start))
    }).collect();
    format!("# Context on the canvas (line-numbered; the \"NNNN| \" prefix is not part of the code)\n\n{}", parts.join("\n\n"))
}

pub fn system_prompt(tree: &[String], web: bool, focus: Option<&str>) -> String {
    let mut listing = tree.iter().take(400).cloned().collect::<Vec<_>>().join("\n");
    if tree.len() > 400 { listing.push_str(&format!("\n… ({} more)", tree.len() - 400)); }
    let mut parts = vec![
        "You are CodeCanvas, a coding assistant embedded in a visual canvas where code lives as files and \"snippets\" (movable fragments of code).".to_string(),
        "Be concise and precise. Prefer showing code as snippet blocks over describing it.".to_string(),
    ];
    if let Some(f) = focus { parts.push(format!("The user is currently focused on: {f}. Questions refer to it unless stated otherwise.")); }
    parts.push(SNIPPET_PROTOCOL.to_string());
    if web { parts.push(WEB_PROTOCOL.to_string()); }
    parts.push(format!("## Project files\n{listing}"));
    parts.join("\n\n")
}

static TOOL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<(read|search|fetch)\s+(?:path|query|url)\s*=\s*"([^"]+)"\s*/?>"#).unwrap());
static FENCE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)```snippet([^\n]*)\n(.*?)```").unwrap());
static ATTR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(\w+)\s*=\s*"([^"]*)""#).unwrap());

pub fn attrs(info: &str) -> Vec<(String, String)> { ATTR_RE.captures_iter(info).map(|c| (c[1].to_string(), c[2].to_string())).collect() }

pub fn parse_snippets(text: &str) -> Vec<Draft> {
    FENCE_RE.captures_iter(text).map(|c| {
        let a = attrs(&c[1]);
        let get = |k: &str| a.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let replace = get("replace").and_then(|r| { let (x, y) = r.split_once('-')?; let (x, y): (usize, usize) = (x.trim().parse().ok()?, y.trim().parse().ok()?); Some((x, y.max(x))) });
        Draft { title: get("title").unwrap_or_else(|| "Draft".into()), file: get("file"), insert_at: get("insert").and_then(|v| v.parse().ok()), replace, code: c[2].trim_end_matches('\n').to_string(), lang: get("lang") }
    }).collect()
}

pub struct ChatRequest<'a> {
    pub cfg: LlmConfig,
    pub project: &'a Project,
    pub web: Option<WebTools>,
    pub history: Vec<Message>,
    pub context: Vec<ContextItem>,
    pub focus: Option<String>,
    pub max_calls: usize,
    pub cancel: Cancel,
}

/// Removes finished tool tags and holds back a possibly unfinished one at the end of the stream.
fn visible(raw: &str) -> String {
    let hidden = TOOL_RE.replace_all(raw, "").into_owned();
    match hidden.rfind('<') {
        Some(lt) if !hidden[lt..].contains('>') && hidden.len() - lt < 300 => hidden[..lt].to_string(),
        _ => hidden,
    }
}

pub fn run_chat(req: ChatRequest, on_delta: &mut dyn FnMut(&str), on_status: &mut dyn FnMut(&str)) -> Result<(String, Vec<Draft>), String> {
    let tree = req.project.tree();
    let mut system = system_prompt(&tree, req.web.is_some(), req.focus.as_deref());
    let ctx = render_context(&req.context);
    if !ctx.is_empty() { system = format!("{system}\n\n{ctx}"); }
    let mut msgs = vec![Message { role: "system", content: system }];
    msgs.extend(req.history);
    let (mut shown, mut calls) = (String::new(), 0);
    loop {
        let (mut raw, mut emitted, mut thinking) = (String::new(), 0usize, false);
        let out = {
            // two closures need the status callback; thinking is reported through a flag between chunks
            let mut delta = |d: &str| {
                raw.push_str(d);
                let safe = visible(&raw);
                if safe.len() > emitted && safe.is_char_boundary(emitted) { on_delta(&safe[emitted..]); emitted = safe.len(); }
            };
            let mut think = || thinking = true;
            stream_chat(&req.cfg, &msgs, &req.cancel, &mut delta, &mut think)?
        };
        if thinking { on_status("thinking…"); }
        let reqs: Vec<(String, String)> = TOOL_RE.captures_iter(&out).map(|c| (c[1].to_string(), c[2].to_string())).collect();
        let cleaned = TOOL_RE.replace_all(&out, "").into_owned();
        if cleaned.len() > emitted { on_delta(&cleaned[emitted..]); }
        shown.push_str(&cleaned);
        if reqs.is_empty() || calls >= req.max_calls { break; }
        msgs.push(Message { role: "assistant", content: out });
        let (mut results, mut trace) = (Vec::new(), Vec::new());
        for (kind, arg) in reqs.into_iter().take(req.max_calls - calls) {
            calls += 1;
            match (kind.as_str(), &req.web) {
                ("read", _) => {
                    on_status(&format!("reading {arg}"));
                    match req.project.read(&arg) {
                        None => { results.push(format!("<file path=\"{arg}\">NOT FOUND</file>")); trace.push(format!("*couldn't read {arg}*")); }
                        Some(t) => {
                            let t: String = if t.len() > 60000 { format!("{}\n…(truncated)", t.chars().take(60000).collect::<String>()) } else { t };
                            results.push(format!("<file path=\"{arg}\">\n{}\n</file>", numbered(&t, 1)));
                            trace.push(format!("*read {arg}*"));
                        }
                    }
                }
                ("search", Some(w)) => {
                    on_status(&format!("searching the web for “{arg}”"));
                    results.push(format!("<search_results query=\"{arg}\">\n{}\n</search_results>", w.search(&arg)));
                    trace.push(format!("*searched the web for “{arg}”*"));
                }
                ("fetch", Some(w)) => {
                    on_status(&format!("reading {arg}"));
                    results.push(format!("<page url=\"{arg}\">\n{}\n</page>", w.fetch_page(&arg)));
                    trace.push(format!("*read {arg}*"));
                }
                (k, _) => results.push(format!("<{k}_result>Web access is not configured.</{k}_result>")),
            }
        }
        msgs.push(Message { role: "user", content: format!("{}\n\nContinue your answer.", results.join("\n\n")) });
        let mut note = String::new();
        if !shown.is_empty() && !shown.ends_with('\n') { note.push('\n'); }
        if !trace.is_empty() { note.push_str(&trace.join("\n")); note.push_str("\n\n"); }
        if !note.is_empty() { on_delta(&note); shown.push_str(&note); }
    }
    let drafts = parse_snippets(&shown);
    Ok((shown.trim().to_string(), drafts))
}

/// Splits a file into explained segments, window by window; `progress` receives the running total.
pub fn explain_file(cfg: &LlmConfig, path: &str, text: &str, cancel: &Cancel, progress: &mut dyn FnMut(&[Segment])) -> Result<Vec<Segment>, String> {
    const WINDOW: usize = 320;
    let lines: Vec<&str> = text.split('\n').collect();
    let mut all = Vec::new();
    let mut off = 0;
    while off < lines.len() {
        let last = lines.len().min(off + WINDOW);
        let prompt = format!(
            "Split this part of `{path}` (lines {}-{last}) into logical, contiguous segments (imports, a function, a class method, a config block…), covering every line from {} to {last} with no gaps or overlaps. Aim for segments of 5-60 lines.\nFor each segment give a short title and a 1-3 sentence explanation of what it does and why.\n\nRespond with ONLY a JSON array, no prose:\n[{{\"start\": {}, \"end\": 12, \"title\": \"…\", \"explanation\": \"…\"}}, …]\n\n```\n{}\n```",
            off + 1, off + 1, off + 1, numbered(&lines[off..last].join("\n"), off + 1));
        let msgs = [Message { role: "system", content: "You are a precise code explainer. You output strict JSON only.".into() }, Message { role: "user", content: prompt }];
        let raw = stream_chat(cfg, &msgs, cancel, &mut |_| {}, &mut || {})?;
        let mut segs = parse_segments(&raw, off + 1, last);
        if segs.is_empty() { segs = fallback_segments(&lines, off, last); }
        all.extend(segs);
        progress(&all);
        off += WINDOW;
    }
    Ok(all)
}

pub fn parse_segments(raw: &str, lo: usize, hi: usize) -> Vec<Segment> {
    let (Some(a), Some(b)) = (raw.find('['), raw.rfind(']')) else { return Vec::new() };
    if b <= a { return Vec::new(); }
    let body = &raw[a..=b];
    let parsed = serde_json::from_str::<Value>(body).or_else(|_| serde_json::from_str::<Value>(&Regex::new(r",\s*([\]}])").unwrap().replace_all(body, "$1")));
    let Ok(Value::Array(arr)) = parsed else { return Vec::new() };
    let mut segs: Vec<Segment> = arr.iter().filter_map(|s| {
        let start = s["start"].as_f64()?.round() as usize;
        let end = s["end"].as_f64()?.round() as usize;
        Some(Segment { start: start.clamp(lo, hi), end: end.clamp(lo, hi), title: s["title"].as_str().unwrap_or("Segment").chars().take(80).collect(), explanation: s["explanation"].as_str().unwrap_or("").to_string() })
    }).filter(|s| s.end >= s.start).collect();
    segs.sort_by_key(|s| s.start);
    let (mut out, mut cursor) = (Vec::new(), lo);
    for mut s in segs {
        if s.end < cursor { continue; }
        s.start = s.start.max(cursor);
        cursor = s.end + 1;
        out.push(s);
    }
    out
}

fn fallback_segments(lines: &[&str], off: usize, last: usize) -> Vec<Segment> {
    let (mut out, mut start) = (Vec::new(), off + 1);
    for i in off..last {
        if (lines[i].trim().is_empty() && i + 1 - start >= 8) || i + 1 - start >= 40 || i == last - 1 {
            out.push(Segment { start, end: i + 1, title: format!("Lines {start}-{}", i + 1), explanation: "(The model did not return a usable breakdown for this part.)".into() });
            start = i + 2;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_snippet_blocks() {
        let d = parse_snippets("x\n```snippet file=\"a.ts\" replace=\"3-5\" title=\"T\"\ncode\n```\n```snippet\nb\n```");
        assert_eq!(d[0], Draft { title: "T".into(), file: Some("a.ts".into()), replace: Some((3, 5)), code: "code".into(), ..Default::default() });
        assert_eq!(d[1].title, "Draft");
    }

    #[test]
    fn hides_tool_tags_while_streaming() {
        assert_eq!(visible("hello <read path=\"a\"/> there"), "hello  there");
        assert_eq!(visible("hello <rea"), "hello ");
        assert_eq!(visible("a < b and <div> ok"), "a < b and <div> ok");
    }

    #[test]
    fn segments_are_clamped_and_never_overlap() {
        let s = parse_segments(r#"sure: [{"start":1,"end":5,"title":"a"},{"start":3,"end":9,"title":"b",},]"#, 2, 8);
        assert_eq!((s[0].start, s[0].end, s[1].start, s[1].end), (2, 5, 6, 8));
    }
}
