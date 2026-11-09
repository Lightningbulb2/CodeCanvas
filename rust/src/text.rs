//! A small text buffer with a caret, selection and undo. Used for file editing, chat boxes and every input.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos { pub line: usize, pub col: usize } // col is a byte offset on a char boundary

#[derive(Clone, Debug)]
pub struct TextBuffer {
    pub lines: Vec<String>,
    pub cur: Pos,
    pub anchor: Option<Pos>,
    undo: Vec<(Vec<String>, Pos)>,
    redo: Vec<(Vec<String>, Pos)>,
}

impl Default for TextBuffer { fn default() -> Self { Self::new(vec![String::new()]) } }

impl TextBuffer {
    pub fn new(lines: Vec<String>) -> Self {
        let lines = if lines.is_empty() { vec![String::new()] } else { lines };
        Self { lines, cur: Pos { line: 0, col: 0 }, anchor: None, undo: Vec::new(), redo: Vec::new() }
    }

    pub fn text(&self) -> String { self.lines.join("\n") }
    pub fn is_empty(&self) -> bool { self.lines.len() == 1 && self.lines[0].is_empty() }
    pub fn set_text(&mut self, t: &str) { *self = Self::new(t.split('\n').map(String::from).collect()); }

    fn checkpoint(&mut self) {
        self.undo.push((self.lines.clone(), self.cur));
        if self.undo.len() > 300 { self.undo.remove(0); }
        self.redo.clear();
    }
    pub fn undo(&mut self) {
        if let Some((l, c)) = self.undo.pop() { self.redo.push((std::mem::replace(&mut self.lines, l), self.cur)); self.cur = c; self.anchor = None; }
    }
    pub fn redo(&mut self) {
        if let Some((l, c)) = self.redo.pop() { self.undo.push((std::mem::replace(&mut self.lines, l), self.cur)); self.cur = c; self.anchor = None; }
    }

    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        if a == self.cur { return None; }
        Some((a.min(self.cur), a.max(self.cur)))
    }

    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection()?;
        if a.line == b.line { return Some(self.lines[a.line][a.col..b.col].to_string()); }
        let mut out = self.lines[a.line][a.col..].to_string();
        for l in &self.lines[a.line + 1..b.line] { out.push('\n'); out.push_str(l); }
        out.push('\n');
        out.push_str(&self.lines[b.line][..b.col]);
        Some(out)
    }

    fn remove_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else { self.anchor = None; return false };
        let tail = self.lines[b.line][b.col..].to_string();
        self.lines.drain(a.line + 1..=b.line);
        self.lines[a.line].truncate(a.col);
        self.lines[a.line].push_str(&tail);
        self.cur = a;
        self.anchor = None;
        true
    }

    pub fn insert(&mut self, text: &str) {
        self.checkpoint();
        self.remove_selection();
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut parts = text.split('\n');
        let tail = self.lines[self.cur.line].split_off(self.cur.col);
        let first = parts.next().unwrap_or("");
        self.lines[self.cur.line].push_str(first);
        self.cur.col += first.len();
        for p in parts {
            self.cur.line += 1;
            self.lines.insert(self.cur.line, p.to_string());
            self.cur.col = p.len();
        }
        self.lines[self.cur.line].push_str(&tail);
    }

    fn prev_col(&self, p: Pos) -> usize { self.lines[p.line][..p.col].char_indices().next_back().map_or(0, |(i, _)| i) }
    fn next_col(&self, p: Pos) -> usize { self.lines[p.line][p.col..].chars().next().map_or(p.col, |c| p.col + c.len_utf8()) }

    pub fn backspace(&mut self) {
        if self.selection().is_some() { self.checkpoint(); self.remove_selection(); return; }
        self.anchor = None;
        let p = self.cur;
        if p.col > 0 {
            self.checkpoint();
            let c = self.prev_col(p);
            self.lines[p.line].replace_range(c..p.col, "");
            self.cur.col = c;
        } else if p.line > 0 {
            self.checkpoint();
            let l = self.lines.remove(p.line);
            self.cur = Pos { line: p.line - 1, col: self.lines[p.line - 1].len() };
            self.lines[p.line - 1].push_str(&l);
        }
    }

    pub fn delete(&mut self) {
        if self.selection().is_some() { self.checkpoint(); self.remove_selection(); return; }
        self.anchor = None;
        let p = self.cur;
        if p.col < self.lines[p.line].len() {
            self.checkpoint();
            let c = self.next_col(p);
            self.lines[p.line].replace_range(p.col..c, "");
        } else if p.line + 1 < self.lines.len() {
            self.checkpoint();
            let l = self.lines.remove(p.line + 1);
            self.lines[p.line].push_str(&l);
        }
    }

    fn mark(&mut self, extend: bool) {
        if extend { if self.anchor.is_none() { self.anchor = Some(self.cur); } } else { self.anchor = None; }
    }

    pub fn left(&mut self, extend: bool, word: bool) {
        if !extend { if let Some((a, _)) = self.selection() { self.cur = a; self.anchor = None; return; } }
        self.mark(extend);
        let p = self.cur;
        if p.col == 0 { if p.line > 0 { self.cur = Pos { line: p.line - 1, col: self.lines[p.line - 1].len() }; } return; }
        if !word { self.cur.col = self.prev_col(p); return; }
        let before = &self.lines[p.line][..p.col];
        let trimmed = before.trim_end();
        self.cur.col = trimmed.rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + trimmed[i..].chars().next().unwrap().len_utf8());
    }

    pub fn right(&mut self, extend: bool, word: bool) {
        if !extend { if let Some((_, b)) = self.selection() { self.cur = b; self.anchor = None; return; } }
        self.mark(extend);
        let p = self.cur;
        let line = &self.lines[p.line];
        if p.col == line.len() { if p.line + 1 < self.lines.len() { self.cur = Pos { line: p.line + 1, col: 0 }; } return; }
        if !word { self.cur.col = self.next_col(p); return; }
        let rest = &line[p.col..];
        let skip = rest.len() - rest.trim_start().len();
        let after = &rest[skip..];
        self.cur.col = p.col + skip + after.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(after.len()).max(after.chars().next().map_or(0, char::len_utf8).min(1));
        while !self.lines[p.line].is_char_boundary(self.cur.col) { self.cur.col += 1; }
    }

    fn col_at_chars(&self, line: usize, chars: usize) -> usize {
        self.lines[line].char_indices().nth(chars).map_or(self.lines[line].len(), |(i, _)| i)
    }
    fn chars_at(&self, p: Pos) -> usize { self.lines[p.line][..p.col].chars().count() }

    pub fn up(&mut self, extend: bool) {
        self.mark(extend);
        if self.cur.line == 0 { self.cur.col = 0; return; }
        let c = self.chars_at(self.cur);
        self.cur.line -= 1;
        self.cur.col = self.col_at_chars(self.cur.line, c);
    }
    pub fn down(&mut self, extend: bool) {
        self.mark(extend);
        if self.cur.line + 1 >= self.lines.len() { self.cur.col = self.lines[self.cur.line].len(); return; }
        let c = self.chars_at(self.cur);
        self.cur.line += 1;
        self.cur.col = self.col_at_chars(self.cur.line, c);
    }
    pub fn home(&mut self, extend: bool, doc: bool) { self.mark(extend); if doc { self.cur.line = 0; } self.cur.col = 0; }
    pub fn end(&mut self, extend: bool, doc: bool) { self.mark(extend); if doc { self.cur.line = self.lines.len() - 1; } self.cur.col = self.lines[self.cur.line].len(); }
    pub fn select_all(&mut self) { self.anchor = Some(Pos { line: 0, col: 0 }); self.end(true, true); self.anchor = Some(Pos { line: 0, col: 0 }); }

    /// Places the caret at a line and character column (e.g. from a mouse click).
    pub fn set_cursor(&mut self, line: usize, chars: usize, extend: bool) {
        self.mark(extend);
        self.cur.line = line.min(self.lines.len() - 1);
        self.cur.col = self.col_at_chars(self.cur.line, chars);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_newlines_and_undo() {
        let mut b = TextBuffer::default();
        b.insert("héllo");
        b.insert("\nworld");
        assert_eq!(b.lines, ["héllo", "world"]);
        b.undo();
        assert_eq!(b.lines, ["héllo"]);
        b.redo();
        assert_eq!(b.lines, ["héllo", "world"]);
    }

    #[test]
    fn backspace_respects_multibyte_and_joins_lines() {
        let mut b = TextBuffer::new(vec!["aé".into(), "b".into()]);
        b.set_cursor(0, 2, false);
        b.backspace();
        assert_eq!(b.lines[0], "a");
        b.set_cursor(1, 0, false);
        b.backspace();
        assert_eq!(b.lines, ["ab"]);
        assert_eq!(b.cur, Pos { line: 0, col: 1 });
    }

    #[test]
    fn selection_across_lines_is_replaced() {
        let mut b = TextBuffer::new(vec!["one".into(), "two".into(), "three".into()]);
        b.set_cursor(0, 1, false);
        b.set_cursor(2, 2, true);
        assert_eq!(b.selected_text().unwrap(), "ne\ntwo\nth");
        b.insert("X");
        assert_eq!(b.lines, ["oXree"]);
    }

    #[test]
    fn word_moves() {
        let mut b = TextBuffer::new(vec!["foo bar".into()]);
        b.end(false, false);
        b.left(false, true);
        assert_eq!(b.cur.col, 4);
        b.home(false, false);
        b.right(false, true);
        assert_eq!(b.cur.col, 3);
    }
}
