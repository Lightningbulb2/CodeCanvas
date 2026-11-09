//! Drawing the canvas: panes, cards, lines parting around a dragged block, regions, tabs.

use super::panels::tool;
use super::*;
use crate::model::Card;
use gpui::{AnyElement, Bounds, Div, HighlightStyle, Hsla, MouseButton, Render, SharedString, StyledText, canvas, div, fill, point, prelude::*, rgb, size};

pub(crate) const BG: u32 = 0x1b1d22;
pub(crate) const PANEL: u32 = 0x16181c;
pub(crate) const CARD: u32 = 0x22252b;
pub(crate) const HEADC: u32 = 0x1e2126;
pub(crate) const BORDER: u32 = 0x343942;
pub(crate) const FG: u32 = 0xd6d9de;
pub(crate) const MUTED: u32 = 0x8a9099;
pub(crate) const ACCENT: u32 = 0x4c9aff;
pub(crate) const HOVER: u32 = 0x2a2e36;
pub(crate) const SEL: u32 = 0x264f78;
pub(crate) const OK: u32 = 0x5fd38d;
pub(crate) const BAD: u32 = 0xff6b6b;

pub(crate) fn col(c: u32) -> Hsla { rgb(c).into() }
pub(crate) fn cola(c: u32, a: f32) -> Hsla { let mut h: Hsla = rgb(c).into(); h.a = a; h }

fn tok_color(t: Tok) -> Hsla {
    col(match t { Tok::Keyword => 0xc678dd, Tok::Str => 0x98c379, Tok::Num => 0xd19a66, Tok::Comment => 0x6b7280, Tok::Type => 0xe5c07b, Tok::Func => 0x61afef })
}

/// Expands tabs and remaps highlight ranges to match.
fn display_line(line: &str, spans: Option<&Vec<(Range<usize>, Tok)>>) -> (SharedString, Vec<(Range<usize>, HighlightStyle)>) {
    let style = |t: Tok| HighlightStyle { color: Some(tok_color(t)), ..Default::default() };
    if line.is_empty() { return (" ".into(), Vec::new()); }
    if !line.contains('\t') {
        return (line.to_string().into(), spans.into_iter().flatten().map(|(r, t)| (r.clone(), style(*t))).collect());
    }
    let map = |b: usize| b + line.as_bytes()[..b].iter().filter(|c| **c == b'\t').count() * 3;
    (line.replace('\t', "    ").into(), spans.into_iter().flatten().map(|(r, t)| (map(r.start)..map(r.end), style(*t))).collect())
}

impl CanvasView {
    /// (pixels, whole lines) that original line `i` of `file` is displaced right now.
    fn live_offset(&self, file: u64, i: usize, l: &Live) -> (f32, isize) {
        let lifted = l.src.filter(|s| s.0 == file && !l.ref_mode);
        let (mut px_, mut n, mut rank) = (0., 0isize, i);
        if let Some(s) = lifted { if i > s.2 { px_ -= l.n as f32 * LH; n -= l.n as isize; rank -= l.n; } }
        if l.over == Some(file) && rank >= l.at { px_ += l.n as f32 * LH + 2. * GP; n += l.n as isize; }
        (px_, n)
    }

    fn eased(&mut self, key: (u64, usize), target: f32, window: &mut Window) -> f32 {
        if self.live.is_none() { return target; }
        let cur = self.anim.entry(key).or_insert(target);
        *cur += (target - *cur) * 0.35;
        if (target - *cur).abs() < 0.4 { *cur = target; } else { window.request_animation_frame(); }
        *cur
    }

    fn lines_el(&mut self, card: &Card, pane: u64, sy: f32, vh: f32, opaque: bool, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let v = self.pv(pane).v;
        let z = v.z;
        let id = card.id;
        let editing = self.editing == Some(id);
        let lines: Vec<String> = if editing { self.inputs[&Input::Editor(id)].lines.clone() } else { card.lines.clone() };
        self.set_spans(id, &lines, &card.lang);
        let spans = self.spans[&id].1.clone();
        let live = self.live.take();
        let start_no = if card.is_file() { 1 } else { card.ref_of.and_then(|r| self.doc.region(r)).map_or(1, |r| r.start + 1) };
        let gutter = self.gutter(lines.len()) * z;
        let lh = LH * z;
        let top_px = sy + HEAD * z;
        let first = (((-top_px) / lh).floor().max(0.) as usize).saturating_sub(40);
        let last = ((((vh - top_px) / lh).ceil().max(0.) as usize) + 40).min(lines.len());
        let sel = card.sel_range();
        let mut layer = div().absolute().left_0().top(px(HEAD * z)).w_full().bottom_0().overflow_hidden().font_family(mono()).text_size(px(12.5 * z)).line_height(px(lh))
            .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                let is_file = t.doc.card(id).is_some_and(Card::is_file);
                if is_file || t.editing == Some(id) { t.file_body_down(id, pane, e, cx); }
                else if t.space { t.start_move(id, e, pane, cx); }
                else if e.click_count >= 2 { cx.stop_propagation(); t.exit_edit(); t.enter_edit(id, 0, 0); cx.notify(); }
                else { t.start_move(id, e, pane, cx); }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |t, e: &MouseDownEvent, _, cx| { cx.stop_propagation(); t.card_menu(id, pane, fl(e.position.x), fl(e.position.y)); cx.notify(); }));
        let caret = editing.then(|| self.inputs[&Input::Editor(id)].clone());
        for i in first..last {
            let lifted = live.as_ref().is_some_and(|l| l.src.is_some_and(|s| s.0 == id && !l.ref_mode && i >= s.1 && i <= s.2));
            if lifted { continue; }
            let (target, shift) = live.as_ref().map_or((0., 0), |l| self.live_offset(id, i, l));
            self.live = None;
            let off = { let keep = live.is_some(); if keep { self.live = Some(Live { n: 0, src: None, region: None, over: None, at: 0, ref_mode: false, lock: false, ghost_card: None, lines: Vec::new(), lang: String::new(), title: String::new(), color: 0, gx: 0., gy: 0., gw: 0., pane: 0 }); } let o = self.eased((id, i), target, window); self.live = None; o };
            let (text, hl) = display_line(&lines[i], spans.get(i));
            let selected = sel.is_some_and(|(a, b)| i >= a && i <= b);
            let y = (PAD + i as f32 * LH + off) * z;
            let num = (start_no as isize + i as isize + shift).to_string();
            let mut row = div().absolute().left_0().w_full().top(px(y)).h(px(lh)).flex()
                .when(opaque, |d| d.bg(col(CARD)))
                .when(selected, |d| d.bg(col(SEL)))
                .child(div().w(px(gutter)).flex_none().flex().justify_end().pr(px(10. * z)).text_color(col(MUTED)).child(num))
                .child(div().flex_1().relative().text_color(col(FG)).whitespace_nowrap().overflow_hidden().child(StyledText::new(text).with_highlights(hl)));
            if let Some(b) = &caret {
                let cw = self.char_w(z);
                if let Some((a, e)) = b.selection() {
                    if i >= a.line && i <= e.line {
                        let from = if i == a.line { crate::ui::visual_len(&lines[i][..a.col]) } else { 0 };
                        let to = if i == e.line { crate::ui::visual_len(&lines[i][..e.col]) } else { crate::ui::visual_len(&lines[i]) + 1 };
                        row = row.child(div().absolute().left(px(gutter + from as f32 * cw)).top_0().h_full().w(px((to - from) as f32 * cw)).bg(cola(SEL, 0.6)));
                    }
                }
                if b.cur.line == i {
                    row = row.child(div().absolute().left(px(gutter + crate::ui::visual_len(&lines[i][..b.cur.col]) as f32 * cw)).top_0().h_full().w(px((1.5 * z).max(1.))).bg(col(0xf7c35f)));
                }
            }
            layer = layer.child(row);
        }
        self.live = live;
        layer
    }

    /// The region bars, highlights and resize handles inside a file card.
    fn regions_el(&mut self, card: &Card, pane: u64, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.editing == Some(card.id) { return Vec::new(); }
        let z = self.pv(pane).v.z;
        let mut out = Vec::new();
        for r in self.doc.regions_of(card.id) {
            let gone = self.live.as_ref().is_some_and(|l| !l.ref_mode && l.src.is_some_and(|s| s.0 == card.id && match l.region { Some(id) => id == r.id, None => r.start >= s.1 && r.end <= s.2 }));
            if gone { continue; }
            let off = self.live.as_ref().map_or(0., |l| self.live_offset(card.id, r.start, l).0);
            let y = (HEAD + PAD + r.start as f32 * LH + off) * z;
            let h = (r.end - r.start + 1) as f32 * LH * z;
            let id = r.id;
            let tint = self.hover_tab == Some(id) || self.picked == Some(id);
            let refd = self.doc.is_referenced(id) || self.live.as_ref().is_some_and(|l| l.ref_mode && l.region == Some(id));
            let doomed = self.doomed.contains(&id);
            let mut ov = div().absolute().left_0().top(px(y)).w_full().h(px(h));
            if tint { ov = ov.bg(cola(r.color, 0.13)); }
            if refd { ov = ov.bg(cola(CARD, 0.62)); }
            if doomed { ov = ov.bg(cola(BAD, 0.16)); }
            out.push(ov.into_any_element());
            let edge = (6. * z).max(4.);
            let bar_color = if doomed { BAD } else { r.color };
            let w = if self.selected.contains(&id) { 9. } else { 5. } * z;
            out.push(div().absolute().left_0().top(px(y)).h(px(h)).w(px((16. * z).max(8.)))
                .child(div().absolute().left_0().top_0().h_full().w(px(w)).bg(cola(bar_color, if refd { 0.45 } else { 0.8 })))
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                    if t.space { cx.stop_propagation(); let f = t.doc.region(id).map(|r| r.file).unwrap_or(0); t.start_move(f, e, pane, cx); return; }
                    t.start_region(id, Via::Bar, e, pane, cx);
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |t, e: &MouseDownEvent, _, cx| { cx.stop_propagation(); t.region_menu(id, fl(e.position.x), fl(e.position.y)); cx.notify(); }))
                .child(div().absolute().left_0().top_0().w_full().h(px(edge)).cursor_ns_resize().hover(move |s| s.bg(cola(bar_color, 0.7)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| t.start_edge(id, true, e, cx))))
                .child(div().absolute().left_0().bottom_0().w_full().h(px(edge)).cursor_ns_resize().hover(move |s| s.bg(cola(bar_color, 0.7)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| t.start_edge(id, false, e, cx))))
                .into_any_element());
        }
        out
    }

    fn header_el(&mut self, card: &Card, pane: u64, z: f32, docked: bool, cx: &mut Context<Self>) -> Div {
        let id = card.id;
        let is_file = card.is_file();
        let in_ctx = self.doc.ctx.contains(&id);
        let mut tools = div().flex().gap(px(2. * z));
        let sz = 13. * z;
        if is_file {
            let path = card.path.clone().unwrap();
            let spin = self.explaining.contains_key(&path);
            tools = tools
                .child(tool(if spin { "…" } else { "?" }, sz, MUTED, cx, move |t, _, _| t.explain(path.clone())))
                .child(tool("≡", sz, MUTED, cx, move |t, _, _| t.toggle_notes(id)))
                .child(tool("❏", sz, MUTED, cx, move |t, _, _| t.toggle_chat(id)))
                .child(tool("◎", sz, if in_ctx { OK } else { MUTED }, cx, move |t, _, _| t.toggle_ctx(id)))
                .child(tool("⤓", sz, MUTED, cx, move |t, _, _| t.save_file(id)))
                .child(tool("↗", sz, MUTED, cx, move |t, _, _| t.open_in_editor(id, 1)))
                .child(tool("×", sz, MUTED, cx, move |t, _, _| t.close_card(id)));
        } else {
            let is_ref = card.ref_of.is_some();
            if let Some(r) = card.ref_of { tools = tools.child(tool("↗", sz, MUTED, cx, move |t, _, _| t.focus_card(r))); }
            if card.target.is_some() { tools = tools.child(tool("⤵", sz, MUTED, cx, move |t, _, _| t.apply_to_target(id))); }
            tools = tools
                .child(tool("◎", sz, if in_ctx { OK } else { MUTED }, cx, move |t, _, _| t.toggle_ctx(id)))
                .child(tool("❏", sz, MUTED, cx, move |t, _, _| t.toggle_chat(id)))
                .child(tool(if is_ref { "×" } else { "🗑" }, sz, MUTED, cx, move |t, _, _| t.close_card(id)));
        }
        let dirty = if card.dirty() { " ●" } else { "" };
        let sub = if let Some(r) = card.ref_of.and_then(|r| self.doc.region(r)) {
            format!("  ↗ {} lines {}–{}", self.doc.card(r.file).and_then(|f| f.path.clone()).unwrap_or_default(), r.start + 1, r.end + 1)
        } else if let Some(t) = &card.target {
            let wh = t.replace.map(|(a, b)| format!("replace {a}–{b}")).or(t.insert_at.map(|n| format!("near line {n}"))).unwrap_or_default();
            format!("  suggested for {} {wh}", t.file)
        } else { String::new() };
        let renaming = self.focus_input == Some(Input::Rename(id));
        let title: AnyElement = if renaming { self.input_el(Input::Rename(id), "", 12.5 * z, cx).flex_1().into_any_element() } else {
            div().flex_1().overflow_hidden().whitespace_nowrap().child(format!("{}{dirty}", card.title)).child(div().text_color(col(MUTED)).child(sub)).flex().into_any_element()
        };
        let bg = if is_file { col(HEADC) } else { cola(card.color, 0.14) };
        div().relative().flex().items_center().h(px(HEAD * z)).px(px(10. * z)).bg(bg).text_size(px(12.5 * z)).text_color(col(FG)).when(docked, |d| d.opacity(0.))
            .child(title).child(tools)
            .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                if e.click_count >= 2 && t.doc.card(id).is_some_and(|c| !c.is_file()) { cx.stop_propagation(); t.begin_rename(id); cx.notify(); return; }
                t.start_move(id, e, pane, cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |t, e: &MouseDownEvent, _, cx| { cx.stop_propagation(); t.card_menu(id, pane, fl(e.position.x), fl(e.position.y)); cx.notify(); }))
    }

    fn card_box(&self, x: f32, y: f32, w: f32, h: f32, z: f32, border: Hsla) -> Div {
        div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).bg(col(CARD)).border_1().border_color(border).rounded(px(9. * z)).overflow_hidden()
    }

    fn render_cards(&mut self, pane: u64, vw: f32, vh: f32, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let v = self.pv(pane).v;
        let z = v.z;
        let (over, ghost_card) = self.live.as_ref().map_or((None, None), |l| (l.over, l.ghost_card));
        let live_pane_ok = self.live.as_ref().is_some_and(|l| self.shared || l.pane == pane);
        let mut out: Vec<AnyElement> = Vec::new();
        let mut drawers: Vec<AnyElement> = Vec::new(); // tabs and chats always sit above every card
        let order: Vec<Card> = self.doc.cards.iter().filter(|c| self.in_pane(c, pane)).cloned().collect();
        for c in &order {
            if ghost_card == Some(c.id) && over.is_some() { continue; } // drawn inside its slot
            let (live_n, extra) = self.live.as_ref().map_or((0, 0.), |l| {
                let lifted = l.src.is_some_and(|s| s.0 == c.id) && !l.ref_mode;
                let gap = l.over == Some(c.id);
                ((if gap { l.n as isize } else { 0 }) - (if lifted { l.n as isize } else { 0 }), if gap { 2. * GP } else { 0. })
            });
            let editing_n = (self.editing == Some(c.id)).then(|| self.inputs[&Input::Editor(c.id)].lines.len());
            let n = (editing_n.unwrap_or(c.lines.len()) as isize + live_n).max(1) as f32;
            let h_world = HEAD + PAD * 2. + n * LH + extra;
            let (sx, sy, sw, sh) = (c.x * z + v.x, c.y * z + v.y, c.w * z, h_world * z);
            if sy > vh + 2000. * z || sy + sh < -600. || sx > vw + 900. || sx + sw + 900. * z < 0. { continue; }
            let selected = self.selected.contains(&c.id);
            let border = if self.editing == Some(c.id) { col(0xf7c35f) } else if selected { col(ACCENT) } else if c.is_file() { col(BORDER) } else { cola(c.color, 0.7) };
            if z < FAR_Z {
                let name = if c.ref_of.is_some() { format!("↗ {}", c.title) } else { basename(&c.title).to_string() };
                let color = if c.is_file() { FG } else { c.color };
                out.push(self.card_box(sx, sy, sw, sh, z, border).flex().items_center().justify_center().text_size(px(14.)).text_color(col(color)).child(name).into_any_element());
                continue;
            }
            let id = c.id;
            let docking = over == Some(id) && live_pane_ok;
            let lines = self.lines_el(c, pane, sy, vh, docking, window, cx);
            let regions = self.regions_el(c, pane, cx);
            let header = self.header_el(c, pane, z, false, cx);
            let resize = div().absolute().right_0().bottom_0().size(px(14. * z)).cursor_nwse_resize()
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if let Some(c) = t.doc.card(id) { let w = c.w; t.drag = Some(Drag::Resize { id, sx: fl(e.position.x), ow: w, z }); }
                }));
            let shell = self.card_box(sx, sy, sw, sh, z, border);
            if docking {
                // the file's lines glide over the dragged block, which sits in the gap beneath them
                let lines_overlay = div().absolute().left(px(sx)).top(px(sy)).w(px(sw)).h(px(sh)).overflow_hidden().child(lines).children(regions).child(header);
                out.push(shell.into_any_element());
                out.push(self.ghost_el(pane, v, true, cx).into_any_element());
                out.push(lines_overlay.into_any_element());
            } else {
                out.push(shell.child(lines).children(regions).child(header).child(resize).into_any_element());
            }
            drawers.extend(self.overlays(c, pane, sx, sy, sw, z, cx));
        }
        if live_pane_ok && over.is_none() && self.live.as_ref().is_some_and(|l| l.ghost_card.is_none()) {
            out.push(self.ghost_el(pane, v, false, cx).into_any_element());
        }
        out.extend(drawers);
        out
    }

    /// The block being dragged. Docked over a file it snaps into the padded slot with its header hidden.
    fn ghost_el(&mut self, _pane: u64, v: View, docked: bool, _cx: &mut Context<Self>) -> Div {
        let l = self.live.as_ref().unwrap();
        let z = v.z;
        let (mut x, mut y) = (l.gx, l.gy - HEAD - PAD);
        if docked { if let Some(f) = l.over.and_then(|f| self.doc.card(f)) { x = f.x; y = f.y + l.at as f32 * LH + GP; } }
        else if let Some(c) = l.ghost_card.and_then(|i| self.doc.card(i)) { x = c.x; y = c.y; }
        let lines = l.lines.clone();
        let (lang, title, color, n, gw, at, over, is_sel) = (l.lang.clone(), l.title.clone(), l.color, l.n, l.gw, l.at, l.over, l.title.is_empty());
        let id = u64::MAX;
        self.set_spans(id, &lines, &lang);
        let spans = self.spans[&id].1.clone();
        let gutter = self.gutter(lines.len().max(100)) * z;
        let lh = LH * z;
        let mut body = div().absolute().left_0().top(px(HEAD * z)).w_full().font_family(mono()).text_size(px(12.5 * z)).line_height(px(lh));
        for (i, line) in lines.iter().enumerate().take(400) {
            let (text, hl) = display_line(line, spans.get(i));
            let no = if over.is_some() { at + i + 1 } else { i + 1 };
            body = body.child(div().absolute().left_0().w_full().top(px((PAD + i as f32 * LH) * z)).h(px(lh)).flex().when(is_sel, |d| d.bg(cola(SEL, 0.5)))
                .child(div().w(px(gutter)).flex_none().flex().justify_end().pr(px(10. * z)).text_color(col(MUTED)).child(no.to_string()))
                .child(div().flex_1().text_color(col(FG)).whitespace_nowrap().overflow_hidden().child(StyledText::new(text).with_highlights(hl))));
        }
        let h = (HEAD + PAD * 2. + n as f32 * LH) * z;
        let head = div().h(px(HEAD * z)).px(px(10. * z)).flex().items_center().text_size(px(12.5 * z)).text_color(col(FG)).bg(cola(color, 0.14)).when(docked, |d| d.opacity(0.)).child(title);
        div().absolute().left(px(x * z + v.x)).top(px(y * z + v.y)).w(px(gw * z)).h(px(h)).bg(col(CARD)).border_1().border_color(cola(color, 0.6)).rounded(px(9. * z)).overflow_hidden().child(head).child(body)
    }

    /// The tabs and drawers beside a card.
    fn overlays(&mut self, c: &Card, pane: u64, sx: f32, sy: f32, sw: f32, z: f32, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let x = sx + sw;
        if c.is_file() {
            self.chat_stack(c.id, "Chat".to_string(), ACCENT, false, x, sy + 6. * z, pane, z, cx, &mut out);
            let mut last = 30.;
            if self.editing != Some(c.id) {
                for r in self.doc.regions_of(c.id) {
                    let gone = self.live.as_ref().is_some_and(|l| l.region == Some(r.id) && !l.ref_mode);
                    if gone { continue; }
                    let off = self.live.as_ref().map_or(0., |l| self.live_offset(c.id, r.start, l).0);
                    let top = f32::max(HEAD + PAD + r.start as f32 * LH + off, last + 2.);
                    last = top + TAB_GAP + 22.;
                    self.chat_stack(r.id, r.title.clone(), r.color, true, x, sy + top * z, pane, z, cx, &mut out);
                }
            }
        } else {
            self.chat_stack(c.id, c.title.clone(), c.color, true, x, sy + 8. * z, pane, z, cx, &mut out);
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn chat_stack(&mut self, id: u64, label: String, color: u32, has_draft: bool, x: f32, y: f32, pane: u64, z: f32, cx: &mut Context<Self>, out: &mut Vec<AnyElement>) {
        let Some(talk) = self.doc.talk(id).cloned() else { return };
        let is_region = self.doc.region(id).is_some();
        let has_chat = talk.chat.iter().any(|m| m.role != Role::Note);
        let tab_w = 120. * z;
        let tab = |label: String, y: f32, open: bool, cx: &mut Context<Self>| -> Div {
            div().absolute().left(px(x)).top(px(y)).w(px(tab_w)).h(px(22. * z)).flex().items_center().gap(px(5. * z)).px(px(6. * z)).bg(col(HEADC)).border_1().border_color(col(BORDER)).rounded_r(px(7. * z)).text_size(px(11.5 * z)).text_color(col(MUTED)).whitespace_nowrap().overflow_hidden().hover(|s| s.bg(col(HOVER)))
                .child(div().text_color(col(color)).child(if open { "◀" } else { "▶" })).child(label)
                .on_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
        };
        let mut chat_tab = tab(if has_chat { format!("{label} •") } else { label.clone() }, y, talk.open, cx);
        chat_tab = chat_tab.on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| {
            if is_region && !t.space { t.start_region(id, Via::Tab, e, pane, cx); } else { t.toggle_chat(id); cx.notify(); }
        })).on_mouse_down(MouseButton::Right, cx.listener(move |t, e: &MouseDownEvent, _, cx| { if t.doc.region(id).is_some() { t.region_menu(id, fl(e.position.x), fl(e.position.y)); } cx.notify(); }));
        let chat_tab = chat_tab.id(("tab", id)).on_hover(cx.listener(move |t, h: &bool, _, cx| { if t.doc.region(id).is_some() { t.hover_tab = h.then_some(id); cx.notify(); } }));
        out.push(chat_tab.into_any_element());
        if is_region && self.focus_input == Some(Input::Rename(id)) {
            let inp = self.input_el(Input::Rename(id), "", 11.5 * z, cx);
            out.push(div().absolute().left(px(x)).top(px(y)).w(px(tab_w)).h(px(22. * z)).px(px(6. * z)).bg(col(0x111316)).border_1().border_color(col(ACCENT)).child(inp).into_any_element());
        }
        let mut draft_y = y + TAB_GAP * z;
        if talk.open {
            let panel = self.chat_panel(id, 360. * z, 340. * z, z, cx);
            out.push(panel.absolute().left(px(x + tab_w)).top(px(y)).into_any_element());
            draft_y = y + 346. * z;
        }
        if has_draft {
            let stats = self.doc.draft_stats(id).filter(|(a, d)| *a + *d > 0);
            let label = stats.map_or("✎ Draft".to_string(), |(a, d)| format!("✎ Draft +{a} −{d}"));
            let dt = tab(label, draft_y, talk.draft_open, cx).on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { t.toggle_draft(id); cx.notify(); }));
            out.push(dt.into_any_element());
            if talk.draft_open {
                let d = self.draft_panel(id, 460. * z, z, cx);
                out.push(d.absolute().left(px(x + tab_w)).top(px(draft_y)).into_any_element());
            }
        }
    }

    fn selection_tools(&mut self, pane: u64, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.drag.is_some() || self.editing.is_some() { return None; }
        let v = self.pv(pane).v;
        let c = self.doc.cards.iter().find(|c| c.sel.is_some() && self.in_pane(c, pane))?.clone();
        let (a, b) = c.sel_range()?;
        let id = c.id;
        let (x, y) = (c.x * v.z + v.x + 40., (c.y + HEAD + PAD + (b + 1) as f32 * LH) * v.z + v.y + 6.);
        let spot = (c.x + c.w + 80., c.y + HEAD + PAD + a as f32 * LH - 40.);
        let btn = |label: &'static str, cx: &mut Context<Self>, f: Act| -> Div {
            div().px(px(9.)).py(px(4.)).rounded(px(5.)).cursor_pointer().hover(|s| s.bg(col(HOVER))).whitespace_nowrap().child(label)
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, w, cx| { cx.stop_propagation(); f(t, w, cx); cx.notify(); }))
        };
        let pane_id = c.pane;
        Some(div().absolute().left(px(x)).top(px(y)).flex().gap(px(2.)).p(px(3.)).bg(col(CARD)).border_1().border_color(col(BORDER)).rounded(px(7.)).text_size(px(12.)).text_color(col(FG))
            .child(btn("◆ Snippet", cx, Rc::new(move |t, _, _| { t.make_selection_region(id); })))
            .child(btn("✂ Detach", cx, Rc::new(move |t, _, _| { if let Some(r) = t.make_selection_region(id) { t.doc.detach_region(r, spot.0, spot.1, pane_id); t.touch(); } })))
            .child(btn("↗ Reference", cx, Rc::new(move |t, _, _| { if let Some(r) = t.make_selection_region(id) { t.doc.make_reference(r, spot.0, spot.1, pane_id); t.touch(); } })))
            .child(btn("⧉ Copy out", cx, Rc::new(move |t, _, _| { t.snapshot(); t.doc.copy_out(id, a, b, spot.0, spot.1); if let Some(c) = t.doc.card_mut(id) { c.sel = None; } t.touch(); })))
            .child(btn("? Ask", cx, Rc::new(move |t, _, _| { if let Some(r) = t.make_selection_region(id) { t.toggle_chat(r); t.focus_input = Some(Input::Chat(r)); } })))
            .child(btn("◎ Context", cx, Rc::new(move |t, _, _| { if let Some(r) = t.make_selection_region(id) { t.toggle_ctx(r); } })))
            .into_any_element())
    }

    fn pane_el(&mut self, r: &Rect, multi: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let pane = r.id;
        let v = self.pv(pane).v;
        let cards = self.render_cards(pane, r.w, r.h, window, cx);
        let tools = self.selection_tools(pane, cx);
        let g = { let mut g = 24. * v.z; while g < 20. { g *= 4.; } g };
        let (vx, vy, w, h) = (v.x, v.y, r.w, r.h);
        let dots = canvas(|_, _, _| (), move |b: Bounds<Pixels>, _, window, _| {
            let dot = cola(FG, 0.13);
            let mut x = vx.rem_euclid(g);
            while x < w {
                let mut y = vy.rem_euclid(g);
                while y < h {
                    window.paint_quad(fill(Bounds::new(point(b.origin.x + px(x), b.origin.y + px(y)), size(px(1.5), px(1.5))), dot));
                    y += g;
                }
                x += g;
            }
        }).absolute().size_full();
        let is_active = multi && self.active == pane;
        let marquee = match self.drag {
            Some(Drag::Marquee { pane: p, sx, sy, cx: mx, cy: my, .. }) if p == pane => Some(div().absolute().left(px(sx.min(mx) - r.x)).top(px(sy.min(my) - r.y)).w(px((sx - mx).abs())).h(px((sy - my).abs())).border_1().border_color(col(ACCENT)).bg(cola(ACCENT, 0.2))),
            _ => None,
        };
        let pick_line = self.pick.as_ref().and_then(|p| p.over).and_then(|(f, at)| self.doc.card(f).map(|c| (c.clone(), at))).filter(|(c, _)| self.in_pane(c, pane)).map(|(c, at)| {
            div().absolute().left(px(c.x * v.z + v.x)).top(px((c.y + HEAD + PAD + at as f32 * LH) * v.z + v.y - 1.5)).w(px(c.w * v.z)).h(px(3.)).bg(col(0xc792ea))
        });
        let bar = div().absolute().left(px(22.)).top(px(8.)).flex().gap(px(1.)).p(px(2.)).bg(cola(PANEL, 0.92)).border_1().border_color(col(BORDER)).rounded(px(7.)).text_size(px(13.))
            .child(tool("◫", 13., MUTED, cx, move |t, _, _| { t.active = pane; t.split(true); }))
            .child(tool("⊟", 13., MUTED, cx, move |t, _, _| { t.active = pane; t.split(false); }))
            .when(multi, |d| d.child(tool("×", 13., MUTED, cx, move |t, _, _| t.close_pane(pane))));
        div().id(("pane", pane)).absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).overflow_hidden().bg(col(BG))
            .when(is_active, |d| d.border_1().border_color(cola(ACCENT, 0.55)))
            .on_scroll_wheel(cx.listener(move |t, e: &ScrollWheelEvent, _, cx| t.on_wheel(pane, e, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| t.pane_mouse_down(pane, e, cx)))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |t, e: &MouseDownEvent, _, cx| t.pane_mouse_down(pane, e, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |t, e: &MouseDownEvent, _, cx| { t.active = pane; t.canvas_menu(pane, fl(e.position.x), fl(e.position.y)); cx.notify(); }))
            .child(dots).children(cards).children(marquee).children(pick_line).children(tools).child(bar)
            .into_any_element()
    }
}

impl Render for CanvasView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vs = window.viewport_size();
        self.win_size = (fl(vs.width), fl(vs.height));
        self.step(window);
        for id in self.pending_bottom.drain().collect::<Vec<_>>() { self.scroll_handle(id).scroll_to_bottom(); }
        let (rects, sashes) = self.pane_rects();
        let multi = rects.len() > 1;
        let panes: Vec<AnyElement> = rects.iter().map(|r| self.pane_el(r, multi, window, cx)).collect();
        let sash_els: Vec<AnyElement> = sashes.into_iter().map(|s| {
            let (x, y, w, h, hz) = (s.x, s.y, s.w, s.h, s.horizontal);
            div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).bg(col(BORDER)).hover(|d| d.bg(col(ACCENT))).when(hz, |d| d.cursor_col_resize()).when(!hz, |d| d.cursor_row_resize())
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                    let mut node = &t.layout;
                    for i in &s.path { if let Layout::Split { children, .. } = node { node = &children[*i]; } }
                    if let Layout::Split { sizes, .. } = node {
                        let (sum, a0, pair) = (sizes.iter().sum(), sizes[s.idx - 1], sizes[s.idx - 1] + sizes[s.idx]);
                        t.drag = Some(Drag::Sash { sash: s.clone(), start: fl(if s.horizontal { e.position.x } else { e.position.y }), a0, pair, sum });
                    }
                    cx.stop_propagation();
                })).into_any_element()
        }).collect();
        let top = self.topbar(cx);
        let explorer = self.explorer_open.then(|| self.explorer(cx));
        let chat = self.chat_open.then(|| self.global_chat(cx));
        let ctx = self.context_panel(cx);
        let overlays = self.overlays_ui(cx);
        div().id("root").track_focus(&self.focus).on_key_down(cx.listener(Self::on_key)).on_key_up(cx.listener(Self::on_key_up))
            .on_mouse_move(cx.listener(Self::on_move)).on_mouse_up(MouseButton::Left, cx.listener(Self::on_up)).on_mouse_up(MouseButton::Middle, cx.listener(Self::on_up))
            .relative().size_full().bg(col(BG)).text_color(col(FG)).text_size(px(13.))
            .children(panes).children(sash_els).child(top).children(explorer).children(chat).child(ctx).children(overlays)
    }
}
