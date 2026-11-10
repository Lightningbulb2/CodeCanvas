//! Top bar, explorer, chats, drafts, context list, menus, settings, toasts.

use super::fx::{Rich, ease_out, ext_color, icon, ms_since};
use super::render::*;
use super::*;
use crate::diff::{Op, diff_lines};
use gpui::{AnyElement, Div, FontStyle, FontWeight, HighlightStyle, MouseButton, SharedString, StyledText, Transformation, div, prelude::*, radians, relative};

pub(crate) const SETTING_FIELDS: [&str; 12] = [
    "Provider (llamacpp, ollama, openai, anthropic)", "Endpoint (empty = provider default)", "Model", "Temperature", "Max tokens", "Tool calls per answer",
    "SearXNG URL (empty = web search off)", "Results per search", "Editor command (empty = find Zed)", "Hidden folders (comma separated)", "Explain All file limit", "API key (empty keeps the stored key)",
];

/// A square icon button; `glyph` is usually an [`icon`].
pub(crate) fn tool_with(glyph: impl IntoElement, size: f32, cx: &mut Context<CanvasView>, f: impl Fn(&mut CanvasView, &mut Window, &mut Context<CanvasView>) + 'static) -> Div {
    div().size(px(size * 1.72)).flex_none().flex().items_center().justify_center().rounded(px(4.)).cursor_pointer().hover(|s| s.bg(col(HOVER))).child(glyph)
        .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, w, cx| { cx.stop_propagation(); f(t, w, cx); cx.notify(); }))
}

pub(crate) fn tool(name: &'static str, size: f32, color: u32, cx: &mut Context<CanvasView>, f: impl Fn(&mut CanvasView, &mut Window, &mut Context<CanvasView>) + 'static) -> Div {
    tool_with(icon(name, size, col(color)), size, cx, f)
}

/// A top-bar button: optional icon, optional label.
fn tb(glyph: Option<&'static str>, label: impl Into<SharedString>, active: bool, cx: &mut Context<CanvasView>, f: impl Fn(&mut CanvasView, &mut Window, &mut Context<CanvasView>) + 'static) -> Div {
    let label: SharedString = label.into();
    div().flex().flex_none().items_center().gap(px(6.)).px(px(9.)).h(px(26.)).rounded(px(5.)).border_1().border_color(gpui::transparent_black()).cursor_pointer().whitespace_nowrap()
        .hover(|s| s.bg(col(HOVER)).border_color(col(PANEL_BORDER))).when(active, |d| d.bg(cola(ACCENT, 0.22)).border_color(col(ACCENT)))
        .when_some(glyph, |d, g| d.child(icon(g, 15., col(FG)))).when(!label.is_empty(), |d| d.child(label))
        .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, w, cx| { cx.stop_propagation(); f(t, w, cx); cx.notify(); }))
}

/// The inline part of the chat's small markdown: `code`, **bold**, *italic*.
fn md_inline(s: &str) -> Rich {
    let (mut out, mut hl) = (String::new(), Vec::new());
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let mut span = |open: &str, style: HighlightStyle, out: &mut String| -> Option<usize> {
            let body = rest.strip_prefix(open)?;
            let end = body.find(open).filter(|e| *e > 0 && !body.starts_with(' '))?;
            let start = out.len();
            out.push_str(&body[..end]);
            hl.push((start..out.len(), style));
            Some(end + 2 * open.len())
        };
        let used = span("`", HighlightStyle { background_color: Some(cola(FG, 0.09)), color: Some(col(0xe5c07b)), ..Default::default() }, &mut out)
            .or_else(|| span("**", HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() }, &mut out))
            .or_else(|| span("*", HighlightStyle { font_style: Some(FontStyle::Italic), ..Default::default() }, &mut out));
        match used {
            Some(n) => i += n,
            None => { let c = rest.chars().next().unwrap(); out.push(c); i += c.len_utf8(); }
        }
    }
    if out.is_empty() { out.push(' '); }
    (out.into(), hl)
}

/// Paragraphs, headings and lists of an assistant message.
fn md_blocks(text: &str, z: f32) -> Div {
    let mut d = div().flex().flex_col();
    let line = |s: &str| { let (t, hl) = md_inline(s); StyledText::new(t).with_highlights(hl) };
    let mut gap = false;
    for l in text.trim_matches('\n').split('\n') {
        let t = l.trim_start();
        if t.is_empty() { gap = true; continue; }
        let mut row = div().when(gap, |d| d.mt(px(6. * z)));
        gap = false;
        let hashes = t.chars().take_while(|c| *c == '#').count();
        let numbered = t.find(['.', ')']).filter(|i| *i > 0 && *i <= 3 && t[..*i].chars().all(|c| c.is_ascii_digit()) && t[*i + 1..].starts_with(' '));
        if (1..=4).contains(&hashes) && t[hashes..].starts_with(' ') {
            row = row.mt(px(4. * z)).font_weight(FontWeight::SEMIBOLD).text_size(px(13. * z)).child(line(t[hashes..].trim()));
        } else if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            row = row.flex().gap(px(6. * z)).pl(px(6. * z)).child(div().flex_none().text_color(col(MUTED)).child("•")).child(div().flex_1().min_w_0().child(line(item)));
        } else if let Some(i) = numbered {
            row = row.flex().gap(px(6. * z)).pl(px(6. * z)).child(div().flex_none().text_color(col(MUTED)).child(t[..=i].to_string())).child(div().flex_1().min_w_0().child(line(t[i + 2..].trim_start())));
        } else {
            row = row.child(line(l));
        }
        d = d.child(row);
    }
    d
}

enum Block { Text(String), Code { info: String, body: String, closed: bool } }

fn blocks(src: &str) -> Vec<Block> {
    let (mut out, mut rest) = (Vec::new(), src);
    while let Some(i) = rest.find("```") {
        if i > 0 { out.push(Block::Text(rest[..i].to_string())); }
        let after = &rest[i + 3..];
        let nl = after.find('\n').unwrap_or(after.len());
        let (info, body) = (after[..nl].trim().to_string(), after.get(nl + 1..).unwrap_or(""));
        match body.find("```") {
            Some(e) => { out.push(Block::Code { info, body: body[..e].trim_end_matches('\n').to_string(), closed: true }); rest = &body[e + 3..]; }
            None => { out.push(Block::Code { info, body: body.to_string(), closed: false }); rest = ""; }
        }
    }
    if !rest.is_empty() { out.push(Block::Text(rest.to_string())); }
    out
}

impl CanvasView {
    pub(crate) fn input_el(&mut self, key: Input, placeholder: &str, size: f32, cx: &mut Context<Self>) -> Div {
        let focused = self.focus_input.as_ref() == Some(&key);
        let b = self.inputs.entry(key.clone()).or_default().clone();
        let lh = size * 1.45;
        let cw = self.char_ratio * size;
        let k2 = key.clone();
        let mut d = div().relative().w_full().font_family(mono()).text_size(px(size)).line_height(px(lh)).min_h(px(lh)).cursor_text()
            .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); t.exit_edit_unless(&k2); t.focus_input = Some(k2.clone()); cx.notify(); }));
        if b.is_empty() && !focused { return d.text_color(col(MUTED)).child(placeholder.to_string()); }
        for (i, l) in b.lines.iter().enumerate() {
            let text: SharedString = if l.is_empty() { " ".into() } else { l.clone().into() };
            let mut row = div().relative().h(px(lh)).whitespace_nowrap().child(text);
            if focused && b.cur.line == i {
                let n = l[..b.cur.col].chars().count();
                row = row.child(div().absolute().left(px(n as f32 * cw)).top_0().h_full().w(px(1.5)).bg(col(ACCENT)));
            }
            if let Some((a, e)) = b.selection().filter(|_| focused) {
                if i >= a.line && i <= e.line {
                    let from = if i == a.line { l[..a.col].chars().count() } else { 0 };
                    let to = if i == e.line { l[..e.col].chars().count() } else { l.chars().count() + 1 };
                    row = row.child(div().absolute().left(px(from as f32 * cw)).top_0().h_full().w(px((to - from) as f32 * cw)).bg(cola(SEL, 0.6)));
                }
            }
            d = d.child(row);
        }
        d
    }

    fn exit_edit_unless(&mut self, k: &Input) { if let Some(id) = self.editing { if *k != Input::Editor(id) { self.exit_edit(); } } }

    pub(crate) fn topbar(&mut self, cx: &mut Context<Self>) -> Div {
        let name = self.project.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let z = self.pv(self.active).v.z;
        let explaining = self.explain_queue.is_some();
        let local = ["localhost", "127.0.0.1", "[::1]"].iter().any(|h| self.settings.llm().endpoint.contains(h));
        let model = self.settings.model.clone();
        let (shared, chat_on, help) = (self.shared, self.chat_open, self.show_help);
        let check = div().size(px(13.)).flex_none().flex().items_center().justify_center().rounded(px(3.)).border_1().border_color(col(if shared { ACCENT } else { MUTED })).when(shared, |d| d.bg(col(ACCENT)).child(icon("check", 10., col(0xffffff))));
        div().absolute().left_0().top_0().w_full().h(px(TOP)).flex().items_center().gap(px(8.)).px(px(10.)).bg(col(PANEL)).border_b_1().border_color(col(PANEL_BORDER))
            .on_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
            .child(div().flex().flex_none().items_center().gap(px(7.)).font_weight(FontWeight::SEMIBOLD).child(icon("logo", 16., col(SNIP))).child("CodeCanvas"))
            .child(div().text_color(col(MUTED)).whitespace_nowrap().overflow_hidden().child(name)).child(div().flex_1())
            .when(self.any_dirty(), |d| d.child(tb(Some("save"), "Save all", false, cx, |t, _, _| t.save_all())))
            .child(tb(Some(if explaining { "stop" } else { "sparkle" }), if explaining { "Stop explaining" } else { "Explain All" }, false, cx, |t, _, _| t.explain_all()))
            .child(div().w(px(48.)).flex_none().flex().justify_end().text_color(col(MUTED)).child(format!("{}%", (z * 100.).round())))
            .child(tb(Some("fit"), "", false, cx, |t, _, _| { let a = t.active; t.fit(a); }))
            .child(tb(Some("splitR"), "", false, cx, |t, _, _| t.split(true)))
            .child(tb(Some("splitD"), "", false, cx, |t, _, _| t.split(false)))
            .child(div().flex().flex_none().items_center().gap(px(5.)).px(px(8.)).h(px(26.)).rounded(px(5.)).cursor_pointer().text_size(px(12.)).text_color(col(MUTED)).whitespace_nowrap().hover(|s| s.bg(col(HOVER)).text_color(col(FG)))
                .child(check).child("Shared canvas")
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); t.set_shared(!shared); cx.notify(); })))
            .child(div().flex().flex_none().items_center().gap(px(6.)).px(px(9.)).h(px(26.)).rounded(px(5.)).border_1().border_color(gpui::transparent_black()).cursor_pointer().whitespace_nowrap()
                .hover(|s| s.bg(col(HOVER)).border_color(col(PANEL_BORDER))).when(self.models.is_some(), |d| d.bg(col(HOVER)).border_color(col(PANEL_BORDER)))
                .child(div().size(px(7.)).flex_none().rounded_full().bg(col(if local { OK } else { MUTED }))).child(model)
                .on_mouse_down(MouseButton::Left, cx.listener(|t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); if t.models.is_some() { t.models = None; } else { t.request_models(); } cx.notify(); })))
            .child(tb(Some("chat"), "Chat", chat_on, cx, |t, _, _| t.chat_open = !t.chat_open))
            .child(tb(None, "?", help, cx, |t, _, _| t.show_help = !t.show_help))
            .child(tb(Some("gear"), "", false, cx, |t, _, _| t.open_settings()))
    }

    pub(crate) fn explorer(&mut self, cx: &mut Context<Self>) -> Div {
        let filter = self.inputs.get(&Input::Filter).map(|b| b.text().to_lowercase()).unwrap_or_default();
        let mut list = div().id("tree").flex_1().overflow_y_scroll().px(px(4.)).pb(px(12.)).text_size(px(13.));
        let open: HashSet<String> = self.doc.cards.iter().filter_map(|c| c.path.clone()).collect();
        let row = |depth: usize, cx: &mut Context<Self>| div().h(px(22.)).flex_none().pl(px(6. + depth as f32 * 12.)).pr(px(6.)).flex().items_center().gap(px(5.)).rounded(px(4.)).whitespace_nowrap().overflow_hidden().hover(|s| s.bg(col(HOVER))).cursor_pointer()
            .on_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()));
        let file_row = |p: &str, label: String, depth: usize, is_open: bool, cx: &mut Context<Self>| {
            let (a, b) = (p.to_string(), p.to_string());
            row(depth, cx).child(div().w(px(12.)).flex_none()).child(div().size(px(8.)).flex_none().rounded(px(2.)).bg(col(ext_color(p)))).child(div().text_color(col(if is_open { ACCENT } else { FG })).child(label))
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, _| { t.drag = Some(Drag::Explorer { path: a.clone(), sx: fl(e.position.x), sy: fl(e.position.y), moved: false }); }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |t, e: &MouseDownEvent, _, cx| { t.explorer_menu(b.clone(), fl(e.position.x), fl(e.position.y)); cx.notify(); }))
        };
        if !filter.is_empty() {
            for p in self.tree.iter().filter(|p| p.to_lowercase().contains(&filter)).take(400) { list = list.child(file_row(p, p.clone(), 0, open.contains(p), cx)); }
        } else {
            let tree = self.tree.clone();
            let mut last_dirs: Vec<&str> = Vec::new();
            let mut shown = 0;
            'files: for p in &tree {
                let parts: Vec<&str> = p.split('/').collect();
                let mut prefix = String::new();
                for (d, seg) in parts[..parts.len() - 1].iter().enumerate() {
                    if !prefix.is_empty() { prefix.push('/'); }
                    prefix.push_str(seg);
                    if last_dirs.get(d) != Some(seg) || last_dirs.len() <= d {
                        last_dirs.truncate(d);
                        last_dirs.push(seg);
                        let is_open = self.expanded.contains(&prefix);
                        let turn = self.fx.tween(Tw::Dir(crate::model::hash_str(&prefix)), if is_open { 1. } else { 0. }, 150.);
                        let key = prefix.clone();
                        list = list.child(row(d, cx).child(icon("chev", 12., col(MUTED)).with_transformation(Transformation::rotate(radians(turn * std::f32::consts::FRAC_PI_2)))).child(seg.to_string())
                            .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { if !t.expanded.remove(&key) { t.expanded.insert(key.clone()); } cx.notify(); })));
                    }
                    if !self.expanded.contains(&prefix) { continue 'files; }
                }
                last_dirs.truncate(parts.len() - 1);
                list = list.child(file_row(p, parts[parts.len() - 1].to_string(), parts.len() - 1, open.contains(p), cx));
                shown += 1;
                if shown > 3000 { break; }
            }
        }
        let focused = self.focus_input == Some(Input::Filter);
        let filter_el = self.input_el(Input::Filter, "Filter files…", 12.5, cx);
        div().w(px(EXPLORER_W)).h_full().flex().flex_col().bg(col(PANEL)).border_r_1().border_color(col(PANEL_BORDER))
            .child(div().flex().items_center().pl(px(10.)).pr(px(6.)).pt(px(6.)).pb(px(4.)).text_size(px(11.)).text_color(col(MUTED)).child(div().flex_1().child("EXPLORER")).child(tool("refresh", 13., MUTED, cx, |t, _, _| t.tree = t.project.tree())))
            .child(div().mx(px(8.)).mb(px(6.)).px(px(8.)).py(px(4.)).bg(col(INPUT)).border_1().border_color(col(if focused { ACCENT } else { PANEL_BORDER })).rounded(px(5.)).child(filter_el))
            .child(list)
    }

    fn message_el(&mut self, owner: u64, role: Role, title: Option<String>, content: &str, z: f32, cx: &mut Context<Self>) -> Div {
        match role {
            Role::User => return div().flex().justify_end().child(div().max_w(relative(0.88)).bg(cola(ACCENT, 0.22)).rounded(px(10. * z)).rounded_br(px(2. * z)).px(px(10. * z)).py(px(6. * z)).child(content.to_string())),
            Role::Error => return div().text_color(col(BAD)).child(content.to_string()),
            Role::Note => {
                return div().bg(cola(SNIP, 0.16)).rounded(px(7. * z)).px(px(10. * z)).py(px(7. * z))
                    .child(div().mb(px(2. * z)).text_size(px(10.5 * z)).text_color(col(MUTED)).child(title.unwrap_or("Explanation".into()).to_uppercase())).child(md_blocks(content, z));
            }
            Role::Assistant => {}
        }
        let is_snippet = owner != 0 && !self.doc.card(owner).is_some_and(Card::is_file);
        let mut d = div().flex().flex_col().gap(px(6. * z));
        for (n, b) in blocks(content).into_iter().enumerate() {
            match b {
                Block::Text(t) => if !t.trim().is_empty() { d = d.child(md_blocks(&t, z)); },
                Block::Code { info, closed, .. } if info.starts_with("snippet") => {
                    let a = agent::attrs(&info);
                    let get = |k: &str| a.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
                    let wh = match get("file") { Some(f) => format!("{f}{}", get("replace").or(get("insert")).map_or(String::new(), |x| format!(":{x}"))), None => if is_snippet { "→ draft".into() } else { "unplaced".into() } };
                    d = d.child(div().flex().items_center().gap(px(7. * z)).px(px(9. * z)).py(px(6. * z)).border_1().border_color(cola(SNIP, 0.45)).bg(cola(SNIP, 0.16)).rounded(px(7. * z)).text_size(px(12. * z))
                        .child(div().text_color(col(SNIP)).child("◆")).child(div().flex_1().font_weight(FontWeight::MEDIUM).child(get("title").unwrap_or("Draft snippet".into())))
                        .child(div().text_color(col(MUTED)).font_family(mono()).text_size(px(11. * z)).child(if closed { wh } else { "drafting…".to_string() })));
                }
                Block::Code { info, body, .. } => {
                    let (b1, b2) = (body.clone(), body.clone());
                    let lang = info.split_whitespace().next().unwrap_or("").to_string();
                    let lines: Vec<AnyElement> = body.split('\n').map(|l| div().whitespace_nowrap().child(if l.is_empty() { " ".to_string() } else { l.replace('\t', "    ") }).into_any_element()).collect();
                    let group: SharedString = format!("pre{owner}-{n}").into();
                    let chip = |label: &'static str| div().px(px(6.)).rounded(px(4.)).bg(col(CARD)).border_1().border_color(col(BORDER)).text_size(px(10.5 * z)).cursor_pointer().hover(|s| s.bg(col(HOVER))).child(label);
                    d = d.child(div().group(group.clone()).relative().bg(col(INPUT)).border_1().border_color(col(BORDER)).rounded(px(6. * z)).px(px(10. * z)).py(px(8. * z)).overflow_hidden().font_family(mono()).text_size(px(11.5 * z))
                        .children(lines)
                        .child(div().absolute().right(px(5.)).top(px(5.)).flex().gap(px(4.)).font_family(".SystemUIFont").opacity(0.).group_hover(group, |s| s.opacity(1.))
                            .when(is_snippet, |d| d.child(chip("→ Draft").on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                let o = t.doc.draft_owner(owner);
                                t.doc.talk_mut(o).unwrap().draft = Some(b2.trim_end_matches('\n').split('\n').map(String::from).collect());
                                if let Some(x) = t.doc.talk_mut(owner) { x.draft_open = true; }
                                t.touch();
                                cx.notify();
                            }))))
                            .child(chip("→ Snippet").on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                t.spawn_drafts(vec![Draft { title: "From chat".into(), code: b1.clone(), lang: (!lang.is_empty()).then(|| lang.clone()), ..Default::default() }], owner);
                                cx.notify();
                            })))));
                }
            }
        }
        d
    }

    /// A chat: the global panel (`fixed`, fills `h`) or a snippet's drawer (as tall as its content, up to `h`).
    pub(crate) fn chat_panel(&mut self, owner: u64, w: f32, h: f32, z: f32, fixed: bool, cx: &mut Context<Self>) -> Div {
        let talk = self.doc.talk(owner).cloned().unwrap_or_default();
        let is_file = self.doc.card(owner).is_some_and(Card::is_file);
        let title = self.doc.card(owner).map(|c| basename(&c.title).to_string()).or(self.doc.region(owner).map(|r| r.title.clone())).unwrap_or_else(|| "Chat".into());
        let color = self.doc.region(owner).map(|r| r.color).or(self.doc.card(owner).filter(|c| !c.is_file()).map(|c| c.color)).unwrap_or(ACCENT);
        let handle = self.scroll_handle(owner);
        let mut msgs = div().id(("msgs", owner)).overflow_y_scroll().track_scroll(&handle).flex().flex_col().gap(px(8. * z)).px(px(10. * z)).py(px(8. * z)).line_height(px(12.5 * 1.5 * z))
            .when(fixed, |d| d.flex_1()).when(!fixed, |d| d.flex_auto().min_h(px(40. * z)))
            .on_scroll_wheel(cx.listener(|_, e: &ScrollWheelEvent, _, cx| if !e.modifiers.control { cx.stop_propagation(); }));
        if talk.chat.is_empty() && !self.busy.contains_key(&owner) {
            msgs = msgs.child(div().text_color(col(MUTED)).text_size(px(12. * z)).child(if owner == 0 { "General chat, not tied to any snippet. It sees the global context (top right of the canvas) and can read any project file on its own.\n\nAsk it to draft code and the snippets will appear on the canvas, ready to drag into files." } else if is_file { "Ask anything about this whole file. File sends just this file; Global also sends everything in the context list." } else { "Ask anything about this snippet. Snippet sends only this code; Global also sends everything in the context list. Suggested changes land in the ✎ Draft tab as a diff." }));
        }
        for m in &talk.chat { msgs = msgs.child(self.message_el(owner, m.role, m.title.clone(), &m.content, z, cx)); }
        let busy = self.busy.get(&owner).map(|b| (b.text.clone(), b.status.clone()));
        if let Some((text, status)) = &busy {
            if !text.is_empty() { msgs = msgs.child(self.message_el(owner, Role::Assistant, None, text, z, cx)); }
            self.fx.want_frame = true;
            let beat = ms_since(self.started) / 1000.;
            let dots = (0..3).map(|i| { let p = ((beat - i as f32 * 0.15).fract() * std::f32::consts::TAU).cos(); div().size(px(5. * z)).rounded_full().bg(cola(MUTED, 0.6 - 0.4 * p)) });
            msgs = msgs.child(div().flex().items_center().gap(px(3. * z)).py(px(4. * z)).children(dots).when(!status.is_empty(), |d| d.child(div().ml(px(6. * z)).text_color(col(MUTED)).text_size(px(11. * z)).italic().child(status.clone()))));
        }
        let focused = self.focus_input == Some(Input::Chat(owner));
        let input = self.input_el(Input::Chat(owner), if owner == 0 { "Ask about the project…" } else { "Ask…" }, 12.5 * z, cx);
        let running = busy.is_some();
        let global = talk.global;
        let head = if owner == 0 {
            let n = self.doc.ctx.len();
            div().flex().items_center().gap(px(6.)).px(px(10.)).py(px(6.)).border_b_1().border_color(col(PANEL_BORDER))
                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child("Chat")).child(div().text_color(col(MUTED)).text_size(px(11.)).child(if n > 0 { format!("{n} item{} in context", if n == 1 { "" } else { "s" }) } else { "no context".into() }))
                .child(tool("reset", 13., MUTED, cx, move |t, _, _| t.reset_chat(owner)))
        } else {
            let seg = |label: &'static str, on: bool, to: bool, cx: &mut Context<Self>| div().px(px(8. * z)).py(px(2. * z)).cursor_pointer().text_color(col(if on { FG } else { MUTED })).when(on, |d| d.bg(cola(ACCENT, 0.22))).child(label)
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); if let Some(x) = t.doc.talk_mut(owner) { x.global = to; } t.touch(); cx.notify(); }));
            div().flex().items_center().gap(px(6. * z)).px(px(8. * z)).py(px(5. * z)).border_b_1().border_color(col(BORDER)).text_size(px(12. * z))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().font_weight(FontWeight::MEDIUM).child(title))
                .child(div().flex().flex_none().border_1().border_color(col(BORDER)).rounded(px(6. * z)).overflow_hidden().text_size(px(11. * z)).child(seg(if is_file { "File" } else { "Snippet" }, !global, false, cx)).child(seg("Global", global, true, cx)))
                .child(tool("reset", 13. * z, MUTED, cx, move |t, _, _| t.reset_chat(owner)))
                .child(tool("close", 13. * z, MUTED, cx, move |t, _, _| t.toggle_chat(owner)))
        };
        let side = if owner == 0 { PANEL_BORDER } else { BORDER };
        let shell = if fixed { div().w(px(w)).h(px(h)).bg(col(PANEL)).border_l_1().border_color(col(PANEL_BORDER)) }
            else { div().relative().w(px(w)).max_h(px(h)).pl(px(3. * z)).bg(col(CARD)).border_1().border_l_0().border_color(col(BORDER)).rounded_r(px(9. * z)).child(div().absolute().left_0().top_0().bottom_0().w(px(3. * z)).bg(col(color))) };
        shell.flex().flex_col().text_size(px(12.5 * z)).text_color(col(FG))
            .on_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
            .child(head).child(msgs)
            .child(div().flex().flex_none().gap(px(6. * z)).p(px(7. * z)).items_end().border_t_1().border_color(col(side))
                .child(div().flex_1().min_w_0().min_h(px(32. * z)).bg(col(INPUT)).border_1().border_color(col(if focused { ACCENT } else { BORDER })).rounded(px(7. * z)).px(px(9. * z)).py(px(6. * z)).child(input))
                .child(div().size(px(32. * z)).flex_none().flex().items_center().justify_center().rounded(px(7. * z)).bg(col(if running { BAD } else { BTN })).cursor_pointer().child(icon(if running { "stop" } else { "send" }, 15. * z, col(0xffffff)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); if t.busy.contains_key(&owner) { t.stop_chat(owner); } else { t.submit_chat(owner); } cx.notify(); }))))
    }

    pub(crate) fn draft_panel(&mut self, id: u64, w: f32, z: f32, cx: &mut Context<Self>) -> Div {
        let owner = self.doc.draft_owner(id);
        let base = self.doc.base_lines(owner);
        let draft = self.doc.talk(owner).and_then(|t| t.draft.clone()).unwrap_or_else(|| base.clone());
        let lang = self.doc.region(owner).and_then(|r| self.doc.card(r.file)).or(self.doc.card(owner)).map(|c| c.lang.clone()).unwrap_or_default();
        let (hb, hd) = (highlight(&base, &lang), highlight(&draft, &lang));
        let ops = diff_lines(&base, &draft);
        let (add, del) = (ops.iter().filter(|o| matches!(o, Op::Add(_))).count(), ops.iter().filter(|o| matches!(o, Op::Del(_))).count());
        let start = self.doc.region(owner).map_or(1, |r| r.start + 1);
        let color = self.doc.region(owner).map(|r| r.color).or(self.doc.card(owner).map(|c| c.color)).unwrap_or(ACCENT);
        let editing = self.focus_input == Some(Input::DraftEdit(owner)) || self.inputs.contains_key(&Input::DraftEdit(owner));
        let h_handle = self.scroll_handle(owner.wrapping_add(1 << 40));
        let mut body = div().id(("draft", owner)).max_h(px(360. * z)).min_h(px(40. * z)).overflow_y_scroll().track_scroll(&h_handle).font_family(mono()).text_size(px(12. * z)).line_height(px(LH * z))
            .on_scroll_wheel(cx.listener(|_, e: &ScrollWheelEvent, _, cx| if !e.modifiers.control { cx.stop_propagation(); }));
        if editing {
            body = body.bg(col(INPUT)).child(div().px(px(10. * z)).py(px(6. * z)).child(self.input_el(Input::DraftEdit(owner), "", 12. * z, cx)));
        } else {
            if add + del == 0 { body = body.child(div().font_family(".SystemUIFont").px(px(10. * z)).py(px(6. * z)).text_color(col(MUTED)).child("No changes yet. Click Edit, or ask the LLM below; suggested changes show up here as a diff you can Apply.")); }
            for op in ops {
                let (mark, tint, an, bn, line, spans) = match op {
                    Op::Same(a, b) => (" ", None, Some(a), Some(b), base[a].clone(), hb.get(a)),
                    Op::Del(a) => ("−", Some(BAD), Some(a), None, base[a].clone(), hb.get(a)),
                    Op::Add(b) => ("+", Some(OK), None, Some(b), draft[b].clone(), hd.get(b)),
                };
                let (text, hl) = display_line(&line, spans);
                let num = |n: Option<usize>| div().w(px(30. * z)).flex_none().flex().justify_end().pr(px(6. * z)).text_color(cola(MUTED, 0.7)).child(n.map_or(String::new(), |n| (start + n).to_string()));
                let gone = tint == Some(BAD);
                body = body.child(div().flex().whitespace_nowrap().when_some(tint, |d, c| d.bg(cola(c, 0.15))).child(num(an)).child(num(bn))
                    .child(div().w(px(14. * z)).flex_none().text_color(col(tint.unwrap_or(MUTED))).child(mark))
                    .child(div().flex_1().overflow_hidden().when(gone, |d| d.opacity(0.8).line_through()).child(StyledText::new(text).with_highlights(hl))));
            }
        }
        let focused = self.focus_input == Some(Input::Ask(id));
        let ask = self.input_el(Input::Ask(id), "Ask the LLM for a change… (Enter)", 12. * z, cx);
        let dbtn = |label: &'static str, primary: bool| div().px(px(9. * z)).py(px(2. * z)).rounded(px(5. * z)).border_1().border_color(col(if primary { BTN } else { BORDER })).when(primary, |d| d.bg(col(BTN)).text_color(col(0xffffff))).text_size(px(11.5 * z)).cursor_pointer().hover(|s| s.bg(col(if primary { ACCENT } else { HOVER }))).child(label);
        div().relative().w(px(w)).pl(px(3. * z)).flex().flex_col().bg(col(CARD)).border_1().border_l_0().border_color(col(BORDER)).rounded_r(px(9. * z)).text_size(px(12. * z)).text_color(col(FG))
            .child(div().absolute().left_0().top_0().bottom_0().w(px(3. * z)).bg(col(color)))
            .on_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
            .child(div().flex().items_center().gap(px(6. * z)).px(px(8. * z)).py(px(5. * z)).border_b_1().border_color(col(BORDER))
                .child(div().font_weight(FontWeight::MEDIUM).child("✎ Draft"))
                .child(div().flex_1().font_family(mono()).text_size(px(11. * z)).flex().gap(px(4.)).when(add + del == 0, |d| d.font_family(".SystemUIFont").text_color(col(MUTED)).child("no changes")).when(add + del > 0, |d| d.font_weight(FontWeight::SEMIBOLD).child(div().text_color(col(OK)).child(format!("+{add}"))).child(div().text_color(col(BAD)).child(format!("−{del}")))))
                .child(dbtn(if editing { "Done" } else { "Edit" }, false).on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); if t.inputs.contains_key(&Input::DraftEdit(owner)) { t.focus_input = Some(Input::DraftEdit(owner)); t.commit_draft_edit(); } else { t.begin_draft_edit(id); } cx.notify(); })))
                .child(tool("reset", 13. * z, MUTED, cx, move |t, _, _| t.reset_draft(id)))
                .child(dbtn("Apply", true).on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); t.apply_draft(id); cx.notify(); })))
                .child(tool("close", 13. * z, MUTED, cx, move |t, _, _| t.toggle_draft(id))))
            .child(body)
            .child(div().p(px(6. * z)).border_t_1().border_color(col(BORDER)).child(div().bg(col(INPUT)).border_1().border_color(col(if focused { ACCENT } else { BORDER })).rounded(px(7. * z)).px(px(9. * z)).py(px(6. * z)).child(ask)))
    }

    pub(crate) fn global_chat(&mut self, cx: &mut Context<Self>) -> Div {
        let h = self.win_size.1 - TOP;
        self.chat_panel(0, CHAT_W, h, 1., true, cx)
    }

    pub(crate) fn context_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let right = self.side.1 + 10.;
        let n = self.doc.ctx.len();
        let lines: usize = self.doc.ctx.iter().map(|i| self.doc.ctx_lines(*i)).sum();
        let open = self.hist_open;
        let mut d = div().absolute().right(px(right)).top(px(TOP + 10.)).w(px(290.)).bg(cola(PANEL, 0.94)).border_1().border_color(col(PANEL_BORDER)).rounded(px(9.)).shadow(vec![shadow(8., 26., 0.4)]).text_size(px(12.))
            .on_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
            .on_scroll_wheel(cx.listener(|_, _: &ScrollWheelEvent, _, cx| cx.stop_propagation()))
            .child(div().flex().items_center().gap(px(6.)).px(px(10.)).py(px(7.)).cursor_pointer().child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child("Context"))
                .child(div().text_color(col(MUTED)).child(if n > 0 { format!("{n} item{} · {lines} line{}", if n == 1 { "" } else { "s" }, if lines == 1 { "" } else { "s" }) } else { "empty".into() }))
                .child(div().text_color(col(MUTED)).text_size(px(10.)).child(if open { "▴" } else { "▾" }))
                .on_mouse_down(MouseButton::Left, cx.listener(|t, _: &MouseDownEvent, _, cx| { t.hist_open = !t.hist_open; cx.notify(); })));
        if open {
            let mut chips = div().flex().flex_wrap().gap(px(4.)).px(px(9.)).pb(px(7.));
            if n == 0 { chips = chips.child(div().px(px(1.)).text_color(col(MUTED)).child("Use ◎ on a file or snippet to add it.")); }
            for id in self.doc.ctx.clone() {
                let label: String = self.doc.ctx_label(id).chars().take(26).collect();
                chips = chips.child(div().flex().items_center().gap(px(2.)).pl(px(7.)).pr(px(2.)).bg(col(HOVER)).rounded(px(10.)).text_size(px(11.)).cursor_pointer().child(label).child(tool("close", 9., MUTED, cx, move |t, _, _| t.toggle_ctx(id)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { t.focus_card(id); cx.notify(); })));
            }
            let mut log = div().id("hist").max_h(px(170.)).overflow_y_scroll().border_t_1().border_color(col(PANEL_BORDER)).py(px(4.));
            if self.doc.history.is_empty() { log = log.child(div().px(px(10.)).py(px(2.)).text_color(col(MUTED)).child("Context changes will be listed here.")); }
            for ev in self.doc.history.iter().rev().take(120).cloned().collect::<Vec<_>>() {
                let (r, sec) = (ev.reference, ev.at % 86400);
                log = log.child(div().flex().items_center().gap(px(7.)).px(px(10.)).py(px(3.)).cursor_pointer().hover(|s| s.bg(col(HOVER)))
                    .child(div().w(px(12.)).flex_none().flex().justify_center().font_weight(FontWeight::BOLD).text_color(col(if ev.added { OK } else { BAD })).child(if ev.added { "+" } else { "−" }))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(ev.text))
                    .child(div().flex_none().text_color(col(MUTED)).child(format!("{} ln", ev.lines)))
                    .child(div().flex_none().text_color(col(MUTED)).text_size(px(10.5)).child(format!("{:02}:{:02}", sec / 3600, sec / 60 % 60)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { t.focus_card(r); cx.notify(); })));
            }
            d = d.child(chips).child(log);
        }
        d
    }

    // ---------- menus ----------
    fn items(list: Vec<(String, Option<Act>)>) -> Vec<MenuItem> { list.into_iter().map(|(label, act)| MenuItem { label, act }).collect() }

    pub(crate) fn region_menu(&mut self, id: u64, x: f32, y: f32) {
        let Some(r) = self.doc.region(id).cloned() else { return };
        let Some(f) = self.doc.card(r.file).cloned() else { return };
        let spot = (f.x + f.w + 80., f.y + HEAD + PAD + r.start as f32 * LH, f.pane);
        let file = f.id;
        let list: Vec<(String, Option<Act>)> = vec![
            (r.title.clone(), None),
            ((if r.talk.open { "Hide chat" } else { "Open chat" }).into(), Some(Rc::new(move |t, _, _| t.toggle_chat(id)))),
            ((if r.talk.draft_open { "Hide draft" } else { "Open draft" }).into(), Some(Rc::new(move |t, _, _| t.toggle_draft(id)))),
            ((if self.doc.ctx.contains(&id) { "Remove from context" } else { "Add to context" }).into(), Some(Rc::new(move |t, _, _| t.toggle_ctx(id)))),
            ("Rename…".into(), Some(Rc::new(move |t, _, _| t.begin_rename_region(id)))),
            ("Detach (cut out)".into(), Some(Rc::new(move |t, _, _| { t.snapshot(); t.detach_fx(id, spot.0, spot.1, spot.2); t.touch(); }))),
            ("Pop out reference".into(), Some(Rc::new(move |t, _, _| { t.snapshot(); t.reference_fx(id, spot.0, spot.1, spot.2); t.touch(); }))),
            ("Copy out".into(), Some(Rc::new(move |t, _, _| { if let Some(r) = t.doc.region(id).cloned() { t.snapshot(); t.copy_fx(r.file, r.start, r.end, spot.0, spot.1); t.touch(); } }))),
            ("Edit file here".into(), Some(Rc::new(move |t, _, _| { if let Some(r) = t.doc.region(id).cloned() { t.enter_edit(r.file, r.start, 0); } }))),
            ("Dissolve snippet (keep code)".into(), Some(Rc::new(move |t, _, _| { t.snapshot(); t.doc.dissolve(&[id]); t.touch(); }))),
            ("Dissolve all snippets in this file".into(), Some(Rc::new(move |t, _, _| t.dissolve_all(Some(file))))),
            ("Delete snippet and its lines".into(), Some(Rc::new(move |t, _, _| { t.snapshot(); t.doc.delete_region_and_lines(id); t.touch(); }))),
        ];
        self.menu = Some(Menu { x, y, items: Self::items(list), at: Instant::now() });
    }

    fn begin_rename_region(&mut self, id: u64) { self.begin_rename(id); }

    pub(crate) fn card_menu(&mut self, id: u64, pane: u64, x: f32, y: f32) {
        let Some(c) = self.doc.card(id).cloned() else { return };
        let snips: Vec<u64> = self.selected.iter().copied().filter(|i| self.doc.card(*i).is_some_and(|c| !c.is_file())).collect();
        let mut list: Vec<(String, Option<Act>)> = Vec::new();
        if c.is_file() {
            let path = c.path.clone().unwrap();
            let base = basename(&path).to_string();
            if !snips.is_empty() {
                let s2 = snips.clone();
                list.push((format!("{} snippet(s) → {base}", snips.len()), None));
                list.push((format!("Merge into {base}"), Some(Rc::new(move |t, _, _| t.merge_selection(id)))));
                list.push(("Merge at a line I pick…".into(), Some(Rc::new(move |t, _, _| t.start_pick(s2.clone(), Some(id))))));
            }
            if let Some((a, b)) = c.sel_range() {
                let spot = (c.x + c.w + 80., c.y + HEAD + PAD + a as f32 * LH);
                let p = c.pane;
                list.push((format!("Lines {}–{}", a + 1, b + 1), None));
                list.push(("Create snippet from selection".into(), Some(Rc::new(move |t, _, _| { t.make_selection_region(id); }))));
                list.push(("Detach selection as snippet".into(), Some(Rc::new(move |t, _, _| { if let Some(r) = t.make_selection_region(id) { t.detach_fx(r, spot.0, spot.1, p); t.touch(); } }))));
                list.push(("Copy selection out".into(), Some(Rc::new(move |t, _, _| { t.snapshot(); t.copy_fx(id, a, b, spot.0, spot.1); t.touch(); }))));
                list.push(("Ask the LLM about selection".into(), Some(Rc::new(move |t, _, _| { if let Some(r) = t.make_selection_region(id) { t.toggle_chat(r); t.focus_input = Some(Input::Chat(r)); } }))));
            }
            let p2 = path.clone();
            list.push((base, None));
            list.push(((if c.talk.open { "Hide file chat" } else { "Chat about this file" }).into(), Some(Rc::new(move |t, _, _| t.toggle_chat(id)))));
            list.push(("Explain file".into(), Some(Rc::new(move |t, _, _| t.explain(p2.clone())))));
            list.push(("Dissolve all snippets in this file".into(), Some(Rc::new(move |t, _, _| t.dissolve_all(Some(id))))));
            list.push(((if self.doc.ctx.contains(&id) { "Remove from context" } else { "Add file to context" }).into(), Some(Rc::new(move |t, _, _| t.toggle_ctx(id)))));
            list.push(("Edit in canvas".into(), Some(Rc::new(move |t, _, _| t.enter_edit(id, 0, 0)))));
            list.push(("Save".into(), Some(Rc::new(move |t, _, _| t.save_file(id)))));
            list.push(("Open in Zed".into(), Some(Rc::new(move |t, _, _| { let line = t.doc.card(id).and_then(|c| c.sel_range()).map_or(1, |r| r.0 + 1); t.open_in_editor(id, line); }))));
            list.push(("Close".into(), Some(Rc::new(move |t, _, _| t.close_card(id)))));
        } else {
            let group = if snips.contains(&id) { snips.clone() } else { vec![id] };
            let (g1, g2, g3) = (group.clone(), group.clone(), group.clone());
            list.push((if group.len() > 1 { format!("{} snippets", group.len()) } else { c.title.clone() }, None));
            list.push(("Merge into…".into(), Some(Rc::new(move |t, _, _| t.start_pick(g1.clone(), None)))));
            if c.target.is_some() { list.push(("Apply to its suggested location".into(), Some(Rc::new(move |t, _, _| t.apply_to_target(id))))); }
            if group.len() > 1 { list.push(("Combine into one snippet".into(), Some(Rc::new(move |t, _, _| { t.snapshot(); let mut g = g2.clone(); g.sort_by(|a, b| { let y = |i: &u64| t.doc.card(*i).map_or(0., |c| c.y); y(a).partial_cmp(&y(b)).unwrap() }); t.doc.combine(&g); t.touch(); })))); }
            list.push(((if c.talk.open { "Hide chat" } else { "Open chat" }).into(), Some(Rc::new(move |t, _, _| t.toggle_chat(id)))));
            list.push(((if c.talk.draft_open { "Hide draft" } else { "Open draft" }).into(), Some(Rc::new(move |t, _, _| t.toggle_draft(id)))));
            list.push(("Add to context".into(), Some(Rc::new(move |t, _, _| for i in &g3 { if !t.doc.ctx.contains(i) { t.toggle_ctx(*i); } }))));
            list.push(("Rename…".into(), Some(Rc::new(move |t, _, _| t.begin_rename(id)))));
            list.push(((if c.ref_of.is_some() { "Remove reference" } else { "Delete" }).into(), Some(Rc::new(move |t, _, _| t.close_card(id)))));
        }
        let _ = pane;
        self.menu = Some(Menu { x, y, items: Self::items(list), at: Instant::now() });
    }

    pub(crate) fn canvas_menu(&mut self, pane: u64, x: f32, y: f32) {
        let (wx, wy) = self.to_world(pane, x, y);
        let multi = self.views.len() > 1;
        let mut list: Vec<(String, Option<Act>)> = vec![
            ("New snippet here".into(), Some(Rc::new(move |t, _, _| { let id = t.doc.add_floating("New snippet".into(), vec![String::new()], "x.ts".into(), wx, wy, 400., pane, None); t.enter_edit(id, 0, 0); }))),
            ("Explain All".into(), Some(Rc::new(|t, _, _| t.explain_all()))),
            ("Arrange files in this view".into(), Some(Rc::new(move |t, _, _| t.arrange(pane)))),
            ("Dissolve all snippets (every file)".into(), Some(Rc::new(|t, _, _| t.dissolve_all(None)))),
            ("Fit everything".into(), Some(Rc::new(move |t, _, _| t.fit(pane)))),
            ("Split right".into(), Some(Rc::new(|t, _, _| t.split(true)))),
            ("Split down".into(), Some(Rc::new(|t, _, _| t.split(false)))),
        ];
        if multi { list.push(("Close this view".into(), Some(Rc::new(move |t, _, _| t.close_pane(pane))))); }
        list.push(("Undo".into(), Some(Rc::new(|t, _, _| t.do_undo()))));
        list.push(("Redo".into(), Some(Rc::new(|t, _, _| t.do_redo()))));
        self.menu = Some(Menu { x, y, items: Self::items(list), at: Instant::now() });
    }

    pub(crate) fn explorer_menu(&mut self, path: String, x: f32, y: f32) {
        let (a, b, c) = (path.clone(), path.clone(), path);
        self.menu = Some(Menu { x, y, items: Self::items(vec![
            ("Open on canvas".to_string(), Some(Rc::new(move |t: &mut Self, _: &mut Window, _: &mut Context<Self>| t.open(a.clone(), None)) as Act)),
            ("Explain file".to_string(), Some(Rc::new(move |t: &mut Self, _: &mut Window, _: &mut Context<Self>| t.explain(b.clone())) as Act)),
            ("Open in Zed".to_string(), Some(Rc::new(move |t: &mut Self, _: &mut Window, _: &mut Context<Self>| { t.open(c.clone(), None); if let Some(id) = t.doc.file_by_path(&c).map(|f| f.id) { t.open_in_editor(id, 1); } }) as Act)),
        ]), at: Instant::now() });
    }

    pub(crate) fn open_settings(&mut self) {
        let s = self.settings.clone();
        let vals = [s.provider, s.endpoint, s.model, s.temperature.to_string(), s.max_tokens.to_string(), s.max_tool_calls.to_string(), s.searxng_url, s.max_results.to_string(), s.editor_command, s.exclude_dirs, s.explain_all_max_files.to_string(), String::new()];
        for (i, v) in vals.iter().enumerate() { self.input(Input::Setting(i)).set_text(v); }
        self.show_settings = true;
        self.focus_input = Some(Input::Setting(0));
    }

    fn save_settings(&mut self) {
        let get = |t: &Self, i: usize| t.inputs.get(&Input::Setting(i)).map(|b| b.text().trim().to_string()).unwrap_or_default();
        let mut s = self.settings.clone();
        let provider = get(self, 0);
        if !["llamacpp", "ollama", "openai", "anthropic"].contains(&provider.as_str()) { self.toast("Provider must be llamacpp, ollama, openai or anthropic.", true); return; }
        s.provider = provider; s.endpoint = get(self, 1); s.model = get(self, 2);
        s.temperature = get(self, 3).parse().unwrap_or(s.temperature);
        s.max_tokens = get(self, 4).parse().unwrap_or(s.max_tokens);
        s.max_tool_calls = get(self, 5).parse().unwrap_or(s.max_tool_calls);
        s.searxng_url = get(self, 6);
        s.max_results = get(self, 7).parse().unwrap_or(s.max_results);
        s.editor_command = get(self, 8); s.exclude_dirs = get(self, 9);
        s.explain_all_max_files = get(self, 10).parse().unwrap_or(s.explain_all_max_files);
        let key = get(self, 11);
        let r = s.save().and_then(|_| if key.is_empty() { Ok(()) } else { Settings::set_api_key(&key) });
        if let Err(e) = r { self.toast(format!("Couldn't save settings: {e}"), true); return; }
        self.settings = s;
        self.input(Input::Setting(11)).set_text("");
        self.show_settings = false;
        self.focus_input = None;
        self.toast("Settings saved", false);
    }

    pub(crate) fn overlays_ui(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let (right, bottom_gap) = (self.side.1 + 14., 14.);
        let (ax, ay, aw, ah) = self.canvas_area();
        if self.explain_queue.is_some() || !self.explaining.is_empty() {
            let (label, frac) = match &self.explain_queue {
                Some((f, i)) => (format!("Explaining {}  ({}/{})", self.explaining.keys().next().cloned().unwrap_or_default(), (*i).min(f.len()), f.len()), i.saturating_sub(1) as f32 / f.len().max(1) as f32),
                None => (format!("Explaining {}…", self.explaining.keys().cloned().collect::<Vec<_>>().join(", ")), 0.5),
            };
            out.push(div().absolute().top(px(TOP + 10.)).left(px(ax + aw / 2. - 200.)).w(px(400.)).flex().items_center().gap(px(10.)).pl(px(12.)).pr(px(10.)).pt(px(7.)).pb(px(9.)).bg(col(PANEL)).border_1().border_color(col(PANEL_BORDER)).rounded(px(9.)).shadow(vec![shadow(8., 26., 0.4)]).text_size(px(12.))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(label)).child(tb(None, "Stop", false, cx, |t, _, _| t.stop_explain()))
                .child(div().absolute().left(px(10.)).right(px(10.)).bottom(px(4.)).h(px(2.)).rounded(px(2.)).bg(col(HOVER)).overflow_hidden().child(div().h_full().w(relative(frac.clamp(0.02, 1.))).bg(col(SNIP))))
                .into_any_element());
        }
        let empty = self.doc.cards.is_empty();
        if empty {
            out.push(div().absolute().left(px(ax)).top(px(ay + ah / 2. - 60.)).w(px(aw)).flex().flex_col().items_center().gap(px(5.)).text_color(col(MUTED))
                .child(div().mb(px(4.)).text_size(px(19.)).font_weight(FontWeight::MEDIUM).text_color(col(FG)).child("Your canvas is empty")).child("Click a file in the explorer (or drag it here) to place it.").child("Select lines → ◆ Snippet · drag a snippet onto a file to merge it").child("Double-click empty space for a new snippet · try Explain All").into_any_element());
        }
        if self.show_help || empty {
            let kbd = |k: &'static str| div().px(px(4.)).rounded(px(4.)).bg(col(PANEL)).border_1().border_b_2().border_color(col(PANEL_BORDER)).text_size(px(10.5)).child(k).into_any_element();
            let t = |s: &'static str| div().child(s).into_any_element();
            let parts = vec![kbd("Wheel"), t("scroll ·"), kbd("Shift"), t("+wheel sideways ·"), kbd("Ctrl"), t("+wheel zoom · middle-drag pan ·"), kbd("Space"), t("+drag moves any card ·"), kbd("Ctrl"), t("/"), kbd("Shift"), t("+click multi-select · right-click → Merge · double-click to edit ·"),
                kbd("Shift"), t("+drag moves selected lines or a snippet inside its file ·"), kbd("Alt"), t("+drag a snippet for a reference ·"), kbd("Esc"), t("cancels a drag")];
            out.push(div().absolute().left(px(ax + 12.)).bottom(px(10.)).max_w(px((aw - 24.).max(100.))).flex().flex_wrap().items_center().gap(px(4.)).px(px(8.)).py(px(4.)).bg(cola(PANEL, 0.85)).rounded(px(6.)).text_size(px(11.5)).text_color(col(MUTED)).children(parts).into_any_element());
        }
        if let Some(models) = self.models.clone() {
            let cur = self.settings.model.clone();
            let label = |s: String| div().px(px(9.)).py(px(4.)).text_size(px(10.5)).text_color(col(MUTED)).child(s.to_uppercase());
            let sep = || div().h(px(1.)).mx(px(2.)).my(px(5.)).bg(col(PANEL_BORDER));
            let item = |name: String, on: bool| div().px(px(9.)).py(px(5.)).rounded(px(5.)).cursor_pointer().whitespace_nowrap().overflow_hidden().hover(|s| s.bg(col(HOVER))).when(on, |d| d.text_color(col(ACCENT))).child(name);
            let mut d = div().absolute().top(px(TOP + 4.)).right(px(right + 100.)).w(px(280.)).p(px(5.)).flex().flex_col().bg(col(CARD)).border_1().border_color(col(BORDER)).rounded(px(8.)).shadow(vec![shadow(12., 40., 0.53)]).text_size(px(12.5)).on_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()));
            d = d.child(label(format!("{} models", self.settings.provider)));
            if models.is_empty() { d = d.child(div().px(px(9.)).py(px(5.)).text_color(col(MUTED)).child("None listed. Is the server running?")); }
            for m in models { let n = m.clone(); d = d.child(item(m.clone(), m == cur).on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); t.settings.model = n.clone(); let _ = t.settings.save(); t.models = None; cx.notify(); }))); }
            d = d.child(sep()).child(label("Provider".into()));
            for (p, name) in [("ollama", "Ollama (local)"), ("llamacpp", "llama.cpp (llama-server)"), ("openai", "OpenAI-compatible"), ("anthropic", "Anthropic Claude")] {
                d = d.child(item(name.into(), self.settings.provider == p).on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); t.settings.provider = p.into(); let _ = t.settings.save(); t.models = None; cx.notify(); })));
            }
            d = d.child(sep()).child(item("All settings…".into(), false).on_mouse_down(MouseButton::Left, cx.listener(|t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); t.models = None; t.open_settings(); cx.notify(); })));
            out.push(d.into_any_element());
        }
        if self.show_settings {
            let mut form = div().flex().flex_col().gap(px(8.));
            for (i, label) in SETTING_FIELDS.iter().enumerate() {
                let focused = self.focus_input == Some(Input::Setting(i));
                let input = self.input_el(Input::Setting(i), "", 13., cx);
                form = form.child(div().child(div().text_size(px(11.5)).text_color(col(MUTED)).child(*label)).child(div().mt(px(3.)).px(px(8.)).py(px(5.)).bg(col(INPUT)).border_1().border_color(col(if focused { ACCENT } else { BORDER })).rounded(px(5.)).overflow_hidden().child(input)));
            }
            let (w, h) = self.win_size;
            out.push(div().absolute().left_0().top_0().size_full().bg(cola(0, 0.45)).on_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
                .child(div().absolute().left(px(w / 2. - 280.)).top(px((h / 2. - 370.).max(10.))).w(px(560.)).p(px(16.)).flex().flex_col().gap(px(12.)).bg(col(CARD)).border_1().border_color(col(BORDER)).rounded(px(10.)).shadow(vec![shadow(18., 60., 0.6)])
                    .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("Settings")).child(form)
                    .child(div().flex().justify_end().gap(px(6.)).child(tb(None, "Close", false, cx, |t, _, _| { t.show_settings = false; t.focus_input = None; })).child(tb(None, "Save", true, cx, |t, _, _| t.save_settings()))))
                .into_any_element());
        }
        if let Some(Drag::Explorer { path, moved: true, .. }) = &self.drag {
            let (x, y) = self.mouse;
            out.push(div().absolute().left(px(x + 10.)).top(px(y + 6.)).flex().items_center().gap(px(6.)).px(px(8.)).py(px(3.)).bg(col(CARD)).border_1().border_color(col(ACCENT)).rounded(px(5.)).shadow(vec![shadow(8., 24., 0.47)])
                .child(div().size(px(8.)).rounded(px(2.)).bg(col(ext_color(path)))).child(basename(path).to_string()).into_any_element());
        }
        if let Some(m) = &self.menu {
            let p = ease_out(ms_since(m.at) / 120.);
            if p < 1. { self.fx.want_frame = true; }
            let (x, y) = (m.x.min(self.win_size.0 - 270.), m.y.min(self.win_size.1 - m.items.len() as f32 * 29. - 16.).max(0.) + 4. * (1. - p));
            let mut d = div().absolute().left(px(x)).top(px(y)).w(px(260.)).p(px(4.)).flex().flex_col().bg(col(CARD)).border_1().border_color(col(BORDER)).rounded(px(8.)).shadow(vec![shadow(12., 40., 0.6)]).text_size(px(13.)).opacity(p)
                .on_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()));
            for (i, it) in m.items.iter().enumerate() {
                match &it.act {
                    None => {
                        if i > 0 { d = d.child(div().h(px(1.)).mx(px(2.)).my(px(4.)).bg(col(BORDER))); }
                        d = d.child(div().px(px(10.)).pt(px(4.)).pb(px(2.)).text_size(px(10.5)).text_color(col(MUTED)).overflow_hidden().whitespace_nowrap().child(it.label.to_uppercase()));
                    }
                    Some(a) => { let a = a.clone(); d = d.child(div().px(px(10.)).py(px(5.)).rounded(px(5.)).cursor_pointer().hover(|s| s.bg(col(BTN)).text_color(col(0xffffff))).whitespace_nowrap().overflow_hidden().child(it.label.clone())
                        .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, w, cx| { cx.stop_propagation(); t.menu = None; a(t, w, cx); cx.notify(); }))); }
                }
            }
            out.push(d.into_any_element());
        }
        if !self.toasts.is_empty() {
            let mut t = div().absolute().right(px(right)).bottom(px(bottom_gap)).flex().flex_col().items_end().gap(px(6.));
            for (m, err, at) in &self.toasts {
                let age = ms_since(*at);
                let p = ease_out(age / 200.) * (1. - ((age - 4700.) / 300.).clamp(0., 1.));
                if age < 200. || age > 4700. { self.fx.want_frame = true; }
                t = t.child(div().relative().max_w(px(420.)).pl(px(15.)).pr(px(12.)).py(px(8.)).mt(px(4. * (1. - ease_out(age / 200.)))).bg(col(CARD)).border_1().border_color(col(BORDER)).rounded(px(7.)).shadow(vec![shadow(8., 26., 0.47)]).text_size(px(12.5)).opacity(p).overflow_hidden()
                    .child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).bg(col(if *err { BAD } else { ACCENT }))).child(m.clone()));
            }
            out.push(t.into_any_element());
        }
        out
    }

}
