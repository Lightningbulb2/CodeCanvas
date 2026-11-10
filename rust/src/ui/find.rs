//! Find and replace inside one card, modelled on VS Code's find widget.

use super::render::*;
use super::*;
use gpui::{Div, FontWeight, MouseButton, div, prelude::*};
use regex::{Regex, RegexBuilder};

/// One match: a byte range in a line.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Hit { pub line: usize, pub start: usize, pub end: usize }

/// The state of a card's find bar.
#[derive(Default)]
pub(crate) struct Find {
    pub case: bool,
    pub word: bool,
    pub regex: bool,
    pub keep_case: bool,
    pub replace: bool,
    /// "Find in selection": only these lines (inclusive).
    pub within: Option<(usize, usize)>,
    pub cur: usize,
    /// What the hits below were computed from: the query with its options, and a fingerprint of the text.
    key: (String, u64),
    hits: Rc<Vec<Hit>>,
    re: Option<Regex>,
    pub error: Option<String>,
}

const MAX_HITS: usize = 20_000;

/// VS Code's "preserve case": shape the replacement like the text it replaces.
fn like_case(found: &str, with: &str) -> String {
    let letters = || found.chars().filter(|c| c.is_alphabetic());
    if letters().next().is_none() { return with.to_string(); }
    if letters().all(char::is_uppercase) { return with.to_uppercase(); }
    if letters().all(char::is_lowercase) { return with.to_lowercase(); }
    if letters().next().is_some_and(char::is_uppercase) {
        let mut c = with.chars();
        return c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect());
    }
    with.to_string()
}

impl CanvasView {
    /// The card Ctrl+F should search: the one being edited, else the selected one, else the one under the pointer.
    fn find_target(&self) -> Option<(u64, Option<(usize, usize)>)> {
        if let Some(id) = self.editing { return Some((id, None)); }
        let mut picked = self.selected.iter().copied();
        if let (Some(id), None) = (picked.next(), picked.next()) {
            if self.doc.card(id).is_some() { return Some((id, None)); }
            // a selected snippet inside a file: search its file, limited to the snippet's lines
            if let Some(r) = self.doc.region(id) { return Some((r.file, Some((r.start, r.end)))); }
        }
        let r = self.pane_at(self.mouse.0, self.mouse.1)?;
        let (wx, wy) = self.to_world(r.id, self.mouse.0, self.mouse.1);
        self.doc.cards.iter().rev().find(|c| self.in_pane(c, r.id) && wx >= c.x && wx <= c.x + c.w && wy >= c.y && wy <= c.y + self.card_height(c)).map(|c| (c.id, None))
    }

    /// Ctrl+F / Ctrl+H.
    pub(crate) fn open_find_here(&mut self, replace: bool) {
        match self.find_target() {
            Some((id, within)) => { self.open_find(id, replace); if within.is_some() { self.finds.get_mut(&id).unwrap().within = within; } }
            None => self.toast("Select a file or snippet (or point at one) to search in it.", false),
        }
    }

    pub(crate) fn open_find(&mut self, id: u64, replace: bool) {
        // like VS Code, start from the selected text when it sits on one line
        let seed = self.inputs.get(&Input::Editor(id)).filter(|_| self.editing == Some(id)).and_then(|b| b.selected_text()).filter(|t| !t.is_empty() && !t.contains('\n'));
        let f = self.finds.entry(id).or_default();
        f.replace |= replace;
        if let Some(s) = seed { self.input(Input::Find(id)).set_text(&s); }
        let focus = if replace && !self.input(Input::Find(id)).is_empty() { Input::Replace(id) } else { Input::Find(id) };
        self.input(focus.clone()).select_all();
        self.focus_input = Some(focus);
    }

    pub(crate) fn close_find(&mut self, id: u64) {
        self.finds.remove(&id);
        self.focus_input = (self.editing == Some(id)).then_some(Input::Editor(id));
    }

    /// The matches in a card, recomputed only when the query, its options or the text change.
    pub(crate) fn find_hits(&mut self, id: u64) -> Rc<Vec<Hit>> {
        let none = Rc::new(Vec::new());
        let Some(f) = self.finds.get(&id) else { return none };
        let query = self.inputs.get(&Input::Find(id)).map(|b| b.text()).unwrap_or_default();
        let lines: &[String] = match self.inputs.get(&Input::Editor(id)).filter(|_| self.editing == Some(id)) { Some(b) => &b.lines, None => match self.doc.card(id) { Some(c) => &c.lines, None => return none } };
        let key = (format!("{query}\u{0}{}{}{}{:?}", f.case, f.word, f.regex, f.within), fingerprint(lines));
        if f.key == key { return f.hits.clone(); }
        let query_changed = f.key.0 != key.0;
        let (mut re, mut error) = (None, None);
        if !query.is_empty() {
            let mut p = if f.regex { query.clone() } else { regex::escape(&query) };
            if f.word { p = format!(r"\b(?:{p})\b"); }
            match RegexBuilder::new(&p).case_insensitive(!f.case).build() { Ok(r) => re = Some(r), Err(e) => error = Some(e.to_string().lines().last().unwrap_or("invalid pattern").trim().to_string()) }
        }
        let mut hits = Vec::new();
        if let Some(re) = &re {
            let (a, b) = f.within.unwrap_or((0, usize::MAX));
            'scan: for (i, l) in lines.iter().enumerate().skip(a).take_while(|(i, _)| *i <= b) {
                for m in re.find_iter(l).filter(|m| !m.is_empty()) {
                    hits.push(Hit { line: i, start: m.start(), end: m.end() });
                    if hits.len() >= MAX_HITS { break 'scan; }
                }
            }
        }
        let hits = Rc::new(hits);
        let f = self.finds.get_mut(&id).unwrap();
        (f.key, f.hits, f.re, f.error) = (key, hits.clone(), re, error);
        f.cur = if query_changed { 0 } else { f.cur.min(hits.len().saturating_sub(1)) };
        if query_changed && !hits.is_empty() { self.reveal_hit(id); }
        hits
    }

    pub(crate) fn find_step(&mut self, id: u64, dir: isize) {
        let n = self.find_hits(id).len();
        if n == 0 { return; }
        let f = self.finds.get_mut(&id).unwrap();
        f.cur = (f.cur as isize + dir).rem_euclid(n as isize) as usize;
        self.reveal_hit(id);
    }

    /// Scrolls the view so the current match is on screen, and selects it when the card is being edited.
    fn reveal_hit(&mut self, id: u64) {
        let Some(f) = self.finds.get(&id) else { return };
        let Some(h) = f.hits.get(f.cur).copied() else { return };
        if self.editing == Some(id) {
            if let Some(b) = self.inputs.get_mut(&Input::Editor(id)) {
                if b.lines.get(h.line).is_some_and(|l| l.is_char_boundary(h.start) && l.is_char_boundary(h.end)) { b.anchor = Some(crate::text::Pos { line: h.line, col: h.start }); b.cur = crate::text::Pos { line: h.line, col: h.end }; }
            }
        }
        let Some(c) = self.doc.card(id) else { return };
        let (cx_, cy_, pane) = (c.x, c.y + HEAD + PAD + h.line as f32 * LH, if self.shared { self.active } else { c.pane });
        let r = self.rect_of(pane);
        let Some(p) = self.views.get_mut(&pane) else { return };
        let y = cy_ * p.t.z + p.t.y;
        if y < 90. || y > r.h - 60. { p.t.y = r.h * 0.4 - cy_ * p.t.z; p.anchor = (0., 0.); }
        let x = cx_ * p.t.z + p.t.x;
        if x < -200. || x > r.w - 200. { p.t.x = 80. - cx_ * p.t.z; p.anchor = (0., 0.); }
        self.fx.want_frame = true;
    }

    /// What a match is replaced with: `$1`-style groups in regex mode, optionally shaped like the match.
    fn replacement(&self, id: u64, line: &str, h: Hit) -> String {
        let with = self.inputs.get(&Input::Replace(id)).map(|b| b.text()).unwrap_or_default();
        let Some(f) = self.finds.get(&id) else { return with };
        let mut out = String::new();
        match f.re.as_ref().filter(|_| f.regex).and_then(|re| re.captures_at(line, h.start)).filter(|c| c.get(0).is_some_and(|m| m.start() == h.start && m.end() == h.end)) {
            Some(caps) => caps.expand(&with, &mut out),
            None => out = with,
        }
        if f.keep_case { like_case(&line[h.start..h.end], &out) } else { out }
    }

    /// Writes new text for some lines of a card as one undo step. A reference card writes to its source file.
    fn set_lines(&mut self, id: u64, changed: Vec<(usize, String)>) {
        if changed.is_empty() { return; }
        self.snapshot();
        let Some(c) = self.doc.card(id) else { return };
        let source = c.ref_of.and_then(|r| self.doc.region(r)).map(|r| (r.file, r.start));
        let is_file = c.is_file();
        for (line, text) in changed {
            match source {
                Some((file, start)) => self.doc.splice(file, start + line, 1, vec![text]),
                None if is_file => self.doc.splice(id, line, 1, vec![text]),
                None => if let Some(l) = self.doc.card_mut(id).and_then(|c| c.lines_mut().get_mut(line)) { *l = text; },
            }
        }
        self.doc.sync_refs();
        self.touch();
    }

    pub(crate) fn replace_one(&mut self, id: u64) {
        if self.editing == Some(id) { self.exit_edit(); }
        let hits = self.find_hits(id);
        let Some((cur, c)) = self.finds.get(&id).map(|f| f.cur).zip(self.doc.card(id)) else { return };
        let Some(h) = hits.get(cur).copied() else { return };
        let Some(line) = c.lines.get(h.line).cloned() else { return };
        let with = self.replacement(id, &line, h);
        let after = h.start + with.len();
        self.set_lines(id, vec![(h.line, format!("{}{with}{}", &line[..h.start], &line[h.end..]))]);
        // go on to the next match after what was just written, even if the new text matches too
        let hits = self.find_hits(id);
        let next = hits.iter().position(|x| (x.line, x.start) >= (h.line, after)).unwrap_or(0);
        if let Some(f) = self.finds.get_mut(&id) { f.cur = next; }
        self.reveal_hit(id);
    }

    pub(crate) fn replace_all(&mut self, id: u64) {
        if self.editing == Some(id) { self.exit_edit(); }
        let hits = self.find_hits(id);
        let Some(lines) = self.doc.card(id).map(|c| c.lines.clone()) else { return };
        let mut changed: Vec<(usize, String)> = Vec::new();
        for h in hits.iter().rev() { // right to left, so earlier matches on a line keep their offsets
            let Some(src) = lines.get(h.line) else { continue };
            let with = self.replacement(id, src, *h);
            if changed.last().is_none_or(|(l, _)| *l != h.line) { changed.push((h.line, src.clone())); }
            changed.last_mut().unwrap().1.replace_range(h.start..h.end, &with);
        }
        let n = hits.len();
        self.set_lines(id, changed);
        if n > 0 { self.toast(format!("Replaced {n} occurrence{}", if n == 1 { "" } else { "s" }), false); }
    }

    /// "Find in selection": limit the search to the selected lines, or lift the limit.
    fn toggle_within(&mut self, id: u64) {
        let sel = match self.inputs.get(&Input::Editor(id)).filter(|_| self.editing == Some(id)).and_then(|b| b.selection()) {
            Some((a, b)) => Some((a.line, b.line)),
            None => self.doc.card(id).and_then(|c| c.sel_range()),
        };
        let Some(f) = self.finds.get_mut(&id) else { return };
        if f.within.take().is_none() {
            match sel { Some(s) => f.within = Some(s), None => self.toast("Select some lines first, then turn on Find in Selection.", false) }
        }
    }

    /// Keys while the find or replace box has the keyboard. Returns true when the key was used.
    pub(crate) fn find_key(&mut self, id: u64, in_replace: bool, key: &str, m: gpui::Modifiers) -> bool {
        let ctrl = m.control || m.platform;
        match key {
            "escape" => self.close_find(id),
            "enter" if ctrl && (m.alt || in_replace) => self.replace_all(id),
            "enter" if in_replace => self.replace_one(id),
            "enter" => self.find_step(id, if m.shift { -1 } else { 1 }),
            "f3" => self.find_step(id, if m.shift { -1 } else { 1 }),
            "tab" | "down" | "up" => {
                let to_replace = if key == "tab" { !in_replace } else { key == "down" };
                if to_replace { if let Some(f) = self.finds.get_mut(&id) { f.replace = true; } }
                self.focus_input = Some(if to_replace { Input::Replace(id) } else { Input::Find(id) });
            }
            "f" if ctrl => { self.input(Input::Find(id)).select_all(); self.focus_input = Some(Input::Find(id)); }
            "h" if ctrl => { if let Some(f) = self.finds.get_mut(&id) { f.replace = true; } self.focus_input = Some(Input::Replace(id)); }
            "c" | "w" | "r" | "l" | "p" if m.alt => {
                if key == "l" { self.toggle_within(id); }
                else if let Some(f) = self.finds.get_mut(&id) { match key { "c" => f.case = !f.case, "w" => f.word = !f.word, "r" => f.regex = !f.regex, _ => f.keep_case = !f.keep_case } }
            }
            _ => return false,
        }
        true
    }

    /// The bar itself, drawn at the top right of its card.
    pub(crate) fn find_bar(&mut self, id: u64, card_w: f32, z: f32, cx: &mut Context<Self>) -> Option<Div> {
        self.finds.get(&id)?;
        let hits = self.find_hits(id);
        let f = self.finds.get(&id)?;
        let (case, word, regex, keep, replace, within, cur, error) = (f.case, f.word, f.regex, f.keep_case, f.replace, f.within.is_some(), f.cur, f.error.clone());
        let query_empty = self.inputs.get(&Input::Find(id)).is_none_or(|b| b.is_empty());
        let count = if let Some(e) = &error { e.clone() } else if query_empty { String::new() } else if hits.is_empty() { "No results".into() } else { format!("{} of {}{}", cur + 1, hits.len(), if hits.len() >= MAX_HITS { "+" } else { "" }) };
        let bad = error.is_some() || (!query_empty && hits.is_empty());
        let toggle = |label: &'static str, on: bool, cx: &mut Context<Self>, f: fn(&mut CanvasView, u64)| {
            div().flex_none().px(px(4. * z)).h(px(18. * z)).flex().items_center().rounded(px(3. * z)).border_1().border_color(if on { col(ACCENT) } else { gpui::transparent_black() }).when(on, |d| d.bg(cola(ACCENT, 0.22)).text_color(col(FG)))
                .cursor_pointer().hover(|s| s.bg(col(HOVER))).text_size(tpx(10.5 * z)).font_family(mono()).child(label)
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); f(t, id); cx.notify(); }))
        };
        let act = |label: &'static str, cx: &mut Context<Self>, f: fn(&mut CanvasView, u64)| {
            div().flex_none().px(px(5. * z)).h(px(20. * z)).flex().items_center().rounded(px(3. * z)).cursor_pointer().hover(|s| s.bg(col(HOVER)).text_color(col(FG))).text_size(tpx(11.5 * z)).whitespace_nowrap().child(label)
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); f(t, id); cx.notify(); }))
        };
        let boxed = |focused: bool, bad: bool| div().flex_1().min_w_0().flex().items_center().gap(px(2. * z)).pl(px(6. * z)).pr(px(2. * z)).h(px(24. * z)).bg(col(INPUT)).border_1().border_color(col(if bad { BAD } else if focused { ACCENT } else { BORDER })).rounded(px(3. * z)).overflow_hidden();
        let find_focus = self.focus_input == Some(Input::Find(id));
        let find_input = self.input_el(Input::Find(id), "Find", 12. * z, cx);
        let mut col_ = div().flex_1().min_w_0().flex().flex_col().gap(px(4. * z)).child(
            div().flex().items_center().gap(px(3. * z))
                .child(boxed(find_focus, bad).child(div().flex_1().min_w_0().overflow_hidden().child(find_input))
                    .child(toggle("Aa", case, cx, |t, id| { if let Some(f) = t.finds.get_mut(&id) { f.case = !f.case; } }))
                    .child(toggle("ab", word, cx, |t, id| { if let Some(f) = t.finds.get_mut(&id) { f.word = !f.word; } }))
                    .child(toggle(".*", regex, cx, |t, id| { if let Some(f) = t.finds.get_mut(&id) { f.regex = !f.regex; } })))
                .child(div().flex_none().min_w(px(58. * z)).px(px(3. * z)).text_size(tpx(11. * z)).text_color(col(if bad { BAD } else { MUTED })).whitespace_nowrap().overflow_hidden().max_w(px(150. * z)).child(count))
                .child(act("↑", cx, |t, id| t.find_step(id, -1)))
                .child(act("↓", cx, |t, id| t.find_step(id, 1)))
                .child(toggle("≡", within, cx, |t, id| t.toggle_within(id)))
                .child(act("×", cx, |t, id| t.close_find(id))));
        if replace {
            let rep_focus = self.focus_input == Some(Input::Replace(id));
            let rep_input = self.input_el(Input::Replace(id), "Replace", 12. * z, cx);
            col_ = col_.child(div().flex().items_center().gap(px(3. * z))
                .child(boxed(rep_focus, false).child(div().flex_1().min_w_0().overflow_hidden().child(rep_input))
                    .child(toggle("AB", keep, cx, |t, id| { if let Some(f) = t.finds.get_mut(&id) { f.keep_case = !f.keep_case; } })))
                .child(act("Replace", cx, |t, id| t.replace_one(id)))
                .child(act("All", cx, |t, id| t.replace_all(id))));
        }
        let w = (card_w - 28.).clamp(210., 430.) * z;
        Some(div().absolute().right(px(14. * z)).top(px(HEAD * z)).w(px(w)).flex().items_start().gap(px(3. * z)).p(px(4. * z)).bg(col(0x252830)).border_1().border_color(col(BORDER)).rounded_b(px(5. * z)).shadow(vec![shadow(4. * z, 14. * z, 0.5)])
            .text_color(col(MUTED)).font_weight(FontWeight::NORMAL).cursor_default()
            .on_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
            .child(div().flex_none().w(px(16. * z)).h(px(24. * z)).flex().items_center().justify_center().rounded(px(3. * z)).cursor_pointer().hover(|s| s.bg(col(HOVER))).text_size(tpx(10. * z)).child(if replace { "▾" } else { "▸" })
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); if let Some(f) = t.finds.get_mut(&id) { f.replace = !f.replace; } cx.notify(); })))
            .child(col_))
    }
}
