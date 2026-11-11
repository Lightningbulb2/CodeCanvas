//! Small line-based syntax highlighter (the same approach as the web canvas).

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tok { Keyword, Str, Num, Comment, Type, Func }

/// One highlighted run in a line, as byte offsets (12 bytes, against 24 for a `Range<usize>` and a `Tok`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span { pub start: u32, pub end: u32, pub tok: Tok }

impl Span { pub fn range(&self) -> Range<usize> { self.start as usize..self.end as usize } }

/// The highlighting of a whole file, every line's runs in one allocation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spans { items: Vec<Span>, starts: Vec<u32> }

impl Spans {
    /// The runs of line `i`.
    pub fn line(&self, i: usize) -> Option<&[Span]> {
        let (a, b) = (*self.starts.get(i)? as usize, *self.starts.get(i + 1)? as usize);
        Some(&self.items[a..b])
    }
    pub fn len(&self) -> usize { self.starts.len().saturating_sub(1) }
    /// Lines `a..b` on their own.
    pub fn slice(&self, a: usize, b: usize) -> Spans {
        let (a, b) = (a.min(self.len()), b.min(self.len()));
        let (from, to) = (self.starts[a] as usize, self.starts[b] as usize);
        Spans { items: self.items[from..to].to_vec(), starts: self.starts[a..=b].iter().map(|s| s - from as u32).collect() }
    }
    fn push_line(&mut self, runs: Vec<(Range<usize>, Tok)>) {
        if self.starts.is_empty() { self.starts.push(0); }
        self.items.extend(runs.into_iter().filter(|(r, _)| r.end <= u32::MAX as usize).map(|(r, tok)| Span { start: r.start as u32, end: r.end as u32, tok }));
        self.starts.push(self.items.len() as u32);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Family { C, Hash, Dash, Plain }

const KEYWORDS: &[&str] = &[
    "abstract","as","async","await","break","case","catch","class","const","continue","def","default","defer","del","delete","do","elif","else","elseif","end","enum","export","extends","extern","false","final","finally","fn","for","foreach","from","func","function","go","goto","if","impl","implements","import","in","include","interface","is","lambda","let","local","loop","match","mod","module","mut","namespace","new","nil","none","None","not","null","of","or","and","override","package","pass","private","protected","pub","public","raise","readonly","ref","repeat","require","return","self","Self","static","struct","super","switch","template","then","this","throw","trait","true","True","False","try","type","typedef","typeof","union","unsafe","until","use","using","var","virtual","void","volatile","where","while","with","yield",
];

fn family(path: &str) -> Family {
    let ext = path.rsplit('/').next().unwrap_or(path).rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "py" | "rb" | "sh" | "yml" | "yaml" | "toml" | "ps1" | "r" | "pl" | "conf" | "ini" | "scm" => Family::Hash,
        "lua" | "sql" | "hs" => Family::Dash,
        "html" | "xml" | "svg" | "md" | "txt" | "csv" | "" => Family::Plain,
        _ => Family::C,
    }
}

/// Highlights every line; block comments carry across lines. Ranges are byte offsets on char boundaries.
pub fn highlight(lines: &[String], path: &str) -> Spans {
    let fam = family(path);
    let mut in_block = false;
    let mut out = Spans { items: Vec::new(), starts: Vec::with_capacity(lines.len() + 1) };
    out.starts.push(0);
    for l in lines { out.push_line(line(l, fam, &mut in_block)); }
    out.items.shrink_to_fit();
    out
}

fn line(s: &str, fam: Family, in_block: &mut bool) -> Vec<(Range<usize>, Tok)> {
    let mut out = Vec::new();
    if fam == Family::Plain || s.len() > 2000 {
        return out;
    }
    let mut i = 0;
    if *in_block {
        match s.find("*/") {
            None => return vec![(0..s.len(), Tok::Comment)],
            Some(e) => { out.push((0..e + 2, Tok::Comment)); i = e + 2; *in_block = false; }
        }
    }
    let line_comment = match fam { Family::Hash => "#", Family::Dash => "--", _ => "//" };
    let bytes = s.as_bytes();
    while i < s.len() {
        let rest = &s[i..];
        if rest.starts_with(line_comment) {
            out.push((i..s.len(), Tok::Comment));
            break;
        }
        if fam == Family::C && rest.starts_with("/*") {
            match rest[2..].find("*/") {
                None => { out.push((i..s.len(), Tok::Comment)); *in_block = true; break; }
                Some(e) => { let end = i + 2 + e + 2; out.push((i..end, Tok::Comment)); i = end; continue; }
            }
        }
        let c = rest.chars().next().unwrap();
        if matches!(c, '"' | '\'' | '`') {
            let mut j = i + 1;
            while j < s.len() && bytes[j] != c as u8 { j += if bytes[j] == b'\\' { 2 } else { 1 }; }
            let mut end = (j + 1).min(s.len());
            while !s.is_char_boundary(end) { end += 1; }
            out.push((i..end, Tok::Str));
            i = end;
            continue;
        }
        if c.is_ascii_digit() {
            let end = i + rest.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.' || ch == '_')).unwrap_or(rest.len());
            out.push((i..end, Tok::Num));
            i = end;
            continue;
        }
        if c.is_alphabetic() || c == '_' || c == '$' || c == '@' {
            let end = i + rest.find(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '$' || ch == '@')).unwrap_or(rest.len());
            let word = &s[i..end];
            if KEYWORDS.contains(&word) {
                out.push((i..end, Tok::Keyword));
            } else if s[end..].trim_start().starts_with('(') {
                out.push((i..end, Tok::Func));
            } else if word.chars().next().is_some_and(char::is_uppercase) && word.chars().any(char::is_lowercase) {
                out.push((i..end, Tok::Type));
            }
            i = end;
            continue;
        }
        i += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn l(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }
    /// The runs of line `i` the way the tests spell them.
    fn at(h: &Spans, i: usize) -> Vec<(Range<usize>, Tok)> { h.line(i).unwrap().iter().map(|s| (s.range(), s.tok)).collect() }

    #[test]
    fn lua_uses_dash_comments() {
        let h = highlight(&l(&["local x = 1 -- note"]), "a.lua");
        assert_eq!(at(&h, 0)[0], (0..5, Tok::Keyword));
        assert_eq!(at(&h, 0).last().unwrap().1, Tok::Comment);
    }

    #[test]
    fn block_comments_span_lines() {
        let h = highlight(&l(&["/* a", "b", "c */ let"]), "a.ts");
        assert_eq!(at(&h, 0), vec![(0..4, Tok::Comment)]);
        assert_eq!(at(&h, 1), vec![(0..1, Tok::Comment)]);
        assert_eq!(at(&h, 2)[1].1, Tok::Keyword);
    }

    #[test]
    fn non_ascii_never_splits_a_character() {
        let h = highlight(&l(&["\"é", "é = fn('ü')"]), "a.rs");
        for (i, line) in ["\"é", "é = fn('ü')"].iter().enumerate() {
            for (r, _) in at(&h, i) { assert!(line.is_char_boundary(r.start) && line.is_char_boundary(r.end)); }
        }
    }
}
