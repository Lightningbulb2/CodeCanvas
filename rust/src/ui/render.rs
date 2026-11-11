//! Drawing the canvas: panes, cards, lines parting around a dragged block, regions, tabs.

use super::fx::{Rich, ease_out, ext_color, icon, ms_since};
use super::panels::{tool, tool_with};
use super::*;
use crate::model::Card;
use gpui::{
    AnyElement, BorderStyle, Bounds, BoxShadow, Div, Font, FontWeight, HighlightStyle, Hsla, MouseButton, PathBuilder, Render, SharedString, StyledText, TextRun, Transformation, canvas, div,
    fill, point, prelude::*, quad, radians, rgb, size, transparent_black,
};

pub(crate) const BG: u32 = 0x1b1d22;
pub(crate) const PANEL: u32 = 0x16181c;
pub(crate) const PANEL_BORDER: u32 = 0x2a2e35;
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
pub(crate) const WARN: u32 = 0xf7c35f;
pub(crate) const SNIP: u32 = 0xc792ea;
pub(crate) const INPUT: u32 = 0x111316;
pub(crate) const BTN: u32 = 0x2f6fdb;

pub(crate) fn col(c: u32) -> Hsla { rgb(c).into() }
pub(crate) fn cola(c: u32, a: f32) -> Hsla { let mut h: Hsla = rgb(c).into(); h.a = a; h }
/// `color-mix(in srgb, a t, b)`.
pub(crate) fn mix(a: u32, b: u32, t: f32) -> Hsla {
    let ch = |s: u32| { let (x, y) = (((a >> s) & 0xff) as f32, ((b >> s) & 0xff) as f32); (x * t + y * (1. - t)).round() as u32 };
    col((ch(16) << 16) | (ch(8) << 8) | ch(0))
}
/// A soft drop shadow like the CSS `0 y blur #000a`.
pub(crate) fn shadow(y: f32, blur: f32, alpha: f32) -> BoxShadow { BoxShadow { color: cola(0, alpha), offset: point(px(0.), px(y)), blur_radius: px(blur), spread_radius: px(0.) } }
/// A crisp one-pixel ring (`0 0 0 1px color`).
pub(crate) fn ring(color: Hsla) -> BoxShadow { BoxShadow { color, offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(1.) } }

/// A text size in quarter-pixel steps, so zooming reuses a bounded set of rasterised glyphs.
pub(crate) fn tpx(v: f32) -> Pixels { px(((v * 4.).round() / 4.).max(0.25)) }
/// The size of zoomed chrome text (titles, tabs, chats) at zoom `z`, in whole pixels. GPUI reshapes a
/// label and rasterises its glyphs for every new size, so text that follows a zoom in fine steps costs
/// that on every frame; whole pixels mean about a dozen sizes across a whole zoom, and half a pixel of
/// difference doesn't show at these sizes.
pub(crate) fn tz(v: f32, z: f32) -> Pixels { px((v * z).round().max(1.)) }

pub(crate) fn tok_color(t: Tok) -> Hsla {
    col(match t { Tok::Keyword => 0xc678dd, Tok::Str => 0x98c379, Tok::Num => 0xd19a66, Tok::Comment => 0x6b7280, Tok::Type => 0xe5c07b, Tok::Func => 0x61afef })
}

/// Expands tabs and remaps highlight ranges to match.
pub(crate) fn display_line(line: &str, spans: Option<&[crate::highlight::Span]>) -> Rich {
    let style = |t: Tok| HighlightStyle { color: Some(tok_color(t)), font_style: (t == Tok::Comment).then_some(gpui::FontStyle::Italic), ..Default::default() };
    if line.is_empty() { return (" ".into(), Vec::new()); }
    if !line.contains('\t') {
        return (line.to_string().into(), spans.into_iter().flatten().map(|s| (s.range(), style(s.tok))).collect());
    }
    let map = |b: usize| b + line.as_bytes()[..b].iter().filter(|c| **c == b'\t').count() * 3;
    (line.replace('\t', "    ").into(), spans.into_iter().flatten().map(|s| (map(s.start as usize)..map(s.end as usize), style(s.tok))).collect())
}

/// One line of a card, ready to be painted straight onto the card (no layout elements per line).
struct Row { y: f32, dx: f32, num: SharedString, text: SharedString, hl: Vec<(Range<usize>, HighlightStyle)>, bg: [Option<Hsla>; 2], alpha: f32, sel: Option<(f32, f32)>, caret: Option<f32>,
    /// Find matches on this line: (x, width, is the current one).
    found: Vec<(f32, f32, bool)>,
    /// The text of this line is in a tile; only its backgrounds are painted here.
    covered: bool }

/// Lines are shaped once at this size and painted at whatever size the zoom needs, so zooming never
/// reshapes text: the shaped line is found in the cache every frame and only its glyphs are drawn.
const SHAPE_PX: f32 = 12.5;

/// The fonts of a line for shaping: comments in italics, everything else upright. Colours are applied when painting.
fn text_runs(len: usize, hl: &[(Range<usize>, HighlightStyle)], base: &Font, italic: &Font) -> Vec<TextRun> {
    let run = |n: usize, font: &Font| TextRun { len: n, font: font.clone(), color: col(FG), background_color: None, underline: None, strikethrough: None };
    let (mut out, mut at) = (Vec::new(), 0);
    for (r, _) in hl.iter().filter(|(_, st)| st.font_style.is_some()) {
        if r.start < at || r.end > len || r.start >= r.end { continue; }
        if r.start > at { out.push(run(r.start - at, base)); }
        out.push(run(r.end - r.start, italic));
        at = r.end;
    }
    if at < len { out.push(run(len - at, base)); }
    out
}

/// The code font's glyphs for ASCII characters (upright, italic), each found once by shaping that one
/// character. Code is monospaced, so with these a line is drawn by placing glyphs column by column,
/// with no shaping at all (shaping builds a fresh plan per call and was the biggest cost of drawing
/// lines directly).
#[derive(Default)]
pub(crate) struct AsciiGlyphs { found: Vec<Option<Option<(gpui::FontId, gpui::GlyphId)>>>, metrics: Option<(f32, f32)> }

impl AsciiGlyphs {
    fn get(&mut self, ts: &gpui::WindowTextSystem, ch: u8, slanted: bool, base: &Font, italic: &Font) -> Option<(gpui::FontId, gpui::GlyphId)> {
        if self.found.is_empty() { self.found = vec![None; 256]; }
        let slot = &mut self.found[ch as usize + if slanted { 128 } else { 0 }];
        *slot.get_or_insert_with(|| {
            let text: SharedString = (ch as char).to_string().into();
            let run = TextRun { len: 1, font: if slanted { italic.clone() } else { base.clone() }, color: col(FG), background_color: None, underline: None, strikethrough: None };
            let line = ts.layout_line(&text, px(SHAPE_PX), &[run], None);
            line.runs.first().and_then(|r| r.glyphs.first().map(|g| (r.font_id, g.id)))
        })
    }
    /// (ascent, descent) of the code font at [`SHAPE_PX`].
    fn metrics(&mut self, ts: &gpui::WindowTextSystem, base: &Font) -> (f32, f32) {
        *self.metrics.get_or_insert_with(|| { let id = ts.resolve_font(base); (fl(ts.ascent(id, px(SHAPE_PX))), fl(ts.descent(id, px(SHAPE_PX)))) })
    }
}

/// Paints an ASCII line (tabs already expanded) glyph by glyph in columns `cw` apart.
#[allow(clippy::too_many_arguments)]
fn paint_mono(text: &str, hl: &[(Range<usize>, HighlightStyle)], plain: Hsla, alpha: f32, origin: gpui::Point<Pixels>, line_height: Pixels, glyph_px: f32, pos_px: f32, cw: f32,
    glyphs: &mut AsciiGlyphs, base: &Font, italic: &Font, window: &mut Window) {
    let ts = window.text_system().clone();
    let k = pos_px / SHAPE_PX;
    let (ascent, descent) = glyphs.metrics(&ts, base);
    let baseline = px((fl(origin.y) + (fl(line_height) - (ascent + descent) * k) / 2. + ascent * k).round());
    let mut h = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b' ' { continue; }
        while h < hl.len() && i >= hl[h].0.end { h += 1; }
        let style = hl.get(h).filter(|(r, _)| i >= r.start).map(|(_, st)| st);
        let Some((font_id, glyph)) = glyphs.get(&ts, b, style.is_some_and(|st| st.font_style.is_some()), base, italic) else { continue };
        let mut color = style.and_then(|st| st.color).unwrap_or(plain);
        color.a *= alpha;
        let _ = window.paint_glyph(point(origin.x + px(i as f32 * cw), baseline), font_id, glyph, px(glyph_px), color);
    }
}

/// Paints a line shaped at [`SHAPE_PX`]: glyphs are placed for a `pos_px` font (exactly what the zoom
/// asks for, so text glides with its card) and drawn at `glyph_px` (the nearest quarter pixel, so the
/// glyph cache stays small). Each glyph takes its colour from the highlight ranges.
#[allow(clippy::too_many_arguments)]
fn paint_scaled(line: &gpui::LineLayout, hl: &[(Range<usize>, HighlightStyle)], plain: Hsla, alpha: f32, origin: gpui::Point<Pixels>, line_height: Pixels, glyph_px: f32, pos_px: f32, window: &mut Window) {
    let k = pos_px / SHAPE_PX;
    let (ascent, descent) = (fl(line.ascent) * k, fl(line.descent) * k);
    // whole-pixel baselines: otherwise each row rounds on its own and the rows jitter against each other
    let baseline = px((fl(origin.y) + (fl(line_height) - ascent - descent) / 2. + ascent).round());
    let (mut h, size_px) = (0, px(glyph_px));
    for run in &line.runs {
        for g in &run.glyphs {
            while h < hl.len() && g.index >= hl[h].0.end { h += 1; }
            let mut color = hl.get(h).filter(|(r, _)| g.index >= r.start).and_then(|(_, st)| st.color).unwrap_or(plain);
            color.a *= alpha;
            let at = point(origin.x + px(fl(g.position.x) * k), baseline + px(fl(g.position.y) * k));
            let _ = if g.is_emoji { window.paint_emoji(at, run.font_id, g.id, size_px) } else { window.paint_glyph(at, run.font_id, g.id, size_px, color) };
        }
    }
}

fn dir_and_base(path: &str) -> (&str, &str) { path.rfind('/').map_or(("", path), |i| path.split_at(i + 1)) }

impl CanvasView {
    /// (pixels, whole lines) that original line `i` of `file` is displaced right now.
    fn live_offset(&self, file: u64, i: usize, l: &Live) -> (f32, isize) {
        let lifted = l.src.filter(|s| s.0 == file && !l.ref_mode);
        let (mut px_, mut n, mut rank) = (0., 0isize, i);
        if let Some(s) = lifted { if i > s.2 { px_ -= l.n as f32 * LH; n -= l.n as isize; rank -= l.n; } }
        if l.over == Some(file) && rank >= l.at { px_ += l.n as f32 * LH + 2. * GP; n += l.n as isize; }
        (px_, n)
    }

    /// Glides a line toward where the drag in progress wants it.
    fn eased(&mut self, key: (u64, usize), target: f32, dragging: bool) -> f32 {
        if !dragging { return target; }
        let k = self.fx.step(0.35);
        let cur = self.anim.entry(key).or_insert(target);
        *cur += (target - *cur) * k;
        if (target - *cur).abs() < 0.4 { *cur = target; } else { self.fx.want_frame = true; }
        *cur
    }

    /// Where line `i` sits right now: the drag glide (already stepped this frame) plus running effects.
    fn line_shift(&self, file: u64, i: usize) -> f32 {
        let target = self.live.as_ref().map_or(0., |l| self.live_offset(file, i, l).0);
        self.anim.get(&(file, i)).copied().unwrap_or(target) + self.fx.shift(file, i)
    }

    fn lines_el(&mut self, card: &Card, pane: u64, sx: f32, sy: f32, vw: f32, vh: f32, opaque: bool, cx: &mut Context<Self>) -> Div {
        let z = self.pv(pane).v.z;
        let id = card.id;
        let editing = self.editing == Some(id);
        let edited;
        let lines: &[String] = if editing { edited = self.inputs[&Input::Editor(id)].lines.clone(); &edited } else { &card.lines };
        if editing { self.set_spans(id, lines, &card.lang); } else { self.set_spans_shared(id, &card.lines, &card.lang); }
        let spans = self.spans[&id].1.clone();
        let live = self.live.take();
        let dragging = live.is_some();
        let start_no = if card.is_file() { 1 } else { card.ref_of.and_then(|r| self.doc.region(r)).map_or(1, |r| r.start + 1) };
        let gutter = self.gutter(lines.len()) * z;
        let lh = LH * z;
        let top_px = sy + HEAD * z;
        let first = (((-top_px) / lh).floor().max(0.) as usize).saturating_sub(6);
        // the part of the card's width that is on screen, in world units
        let x_range = ((-sx / z).max(0.), ((vw - sx) / z).min(card.w));
        // the lines and width that will be on screen where the view is heading (to fetch cells early)
        let ahead = {
            let t = self.views.get(&pane).map_or(self.pv(pane).v, |p| p.t);
            let (tsx, tsy) = (card.x * t.z + t.x, (card.y + HEAD) * t.z + t.y);
            let tlh = LH * t.z;
            let a = ((-tsy / tlh).floor().max(0.) as usize).min(lines.len());
            let b = ((((vh - tsy) / tlh).ceil().max(0.)) as usize).min(lines.len());
            (a..b, ((-tsx / t.z).max(0.), ((vw - tsx) / t.z).min(card.w)))
        };
        let last = ((((vh - top_px) / lh).ceil().max(0.) as usize) + 6).min(lines.len());
        let mut layer = div().absolute().left_0().top(px(HEAD * z)).w_full().bottom_0().overflow_hidden()
            .when(card.is_file() || editing, |d| d.cursor_text())
            .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                let is_file = t.doc.card(id).is_some_and(Card::is_file);
                if is_file || t.editing == Some(id) { t.file_body_down(id, pane, e, cx); }
                else if t.space { t.start_move(id, e, pane, cx); }
                else if e.click_count >= 2 { cx.stop_propagation(); t.exit_edit(); t.enter_edit(id, 0, 0); cx.notify(); }
                else { t.start_move(id, e, pane, cx); }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |t, e: &MouseDownEvent, _, cx| { cx.stop_propagation(); t.card_menu(id, pane, fl(e.position.x), fl(e.position.y)); cx.notify(); }));
        if let Some((a, b)) = card.sel_range().filter(|_| !editing) {
            let y = (PAD + a as f32 * LH + self.fx.shift(id, a)) * z;
            layer = layer.child(div().absolute().left_0().w_full().top(px(y)).h(px((b - a + 1) as f32 * lh)).bg(cola(SEL, 0.55)).border_l(px((3. * z).max(1.))).border_color(col(ACCENT)));
        }
        let caret = editing.then(|| self.inputs[&Input::Editor(id)].clone());
        // Text comes from cached tiles wherever every line sits at its plain place; a card that is being
        // edited, dragged into or out of, or running a line effect is drawn directly instead.
        let plain = !editing && !opaque && !self.fx.lines.iter().any(|l| l.file == id)
            && !live.as_ref().is_some_and(|l| l.over == Some(id) || (l.src.is_some_and(|s| s.0 == id) && !l.ref_mode));
        let (tiled, tile_draws) = if plain && last > first { self.cells_for(pane, card, &spans, start_no, first, last, x_range, ahead, z) } else { (Vec::new(), Vec::new()) };
        let mut rows: Vec<Row> = Vec::with_capacity(last.saturating_sub(first));
        let (hits, hit_cur) = (self.find_hits(id), self.finds.get(&id).map_or(usize::MAX, |f| f.cur));
        let mut hit = hits.partition_point(|h| h.line < first);
        let cw = self.char_w(z);
        for i in first..last {
            let lifted = live.as_ref().is_some_and(|l| l.src.is_some_and(|s| s.0 == id && !l.ref_mode && i >= s.1 && i <= s.2));
            if lifted { continue; }
            let (target, shift) = live.as_ref().map_or((0., 0), |l| self.live_offset(id, i, l));
            let off = self.eased((id, i), target, dragging) + self.fx.shift(id, i);
            let alpha = self.fx.alpha(id, i);
            if alpha <= 0.01 { continue; }
            let (flash, dx) = (self.fx.flash(id, i), self.fx.dx(id, i));
            let covered = tiled.get(i - first).copied().unwrap_or(false);
            let (text, hl) = if covered { (SharedString::default(), Vec::new()) } else { display_line(&lines[i], spans.line(i)) };
            let mut row = Row { y: (PAD + i as f32 * LH + off) * z, dx: dx * z, num: (start_no as isize + i as isize + shift).to_string().into(), text, hl,
                bg: [opaque.then(|| col(CARD)), (flash > 0.).then(|| cola(OK, flash))], alpha, sel: None, caret: None, found: Vec::new(), covered };
            while hit < hits.len() && hits[hit].line < i { hit += 1; }
            while hit < hits.len() && hits[hit].line == i {
                let h = hits[hit];
                if lines[i].is_char_boundary(h.start) && lines[i].is_char_boundary(h.end) {
                    let from = crate::ui::visual_len(&lines[i][..h.start]);
                    row.found.push((gutter + from as f32 * cw, crate::ui::visual_len(&lines[i][h.start..h.end]) as f32 * cw, hit == hit_cur));
                }
                hit += 1;
            }
            if let Some(b) = &caret {
                if let Some((a, e)) = b.selection() {
                    if i >= a.line && i <= e.line {
                        let from = if i == a.line { crate::ui::visual_len(&lines[i][..a.col]) } else { 0 };
                        let to = if i == e.line { crate::ui::visual_len(&lines[i][..e.col]) } else { crate::ui::visual_len(&lines[i]) + 1 };
                        row.sel = Some((gutter + from as f32 * cw, (to - from) as f32 * cw));
                    }
                }
                if b.cur.line == i { row.caret = Some(gutter + crate::ui::visual_len(&lines[i][..b.cur.col]) as f32 * cw); }
            }
            if !covered || !row.found.is_empty() || row.bg.iter().any(Option::is_some) { rows.push(row); }
        }
        let (font_px, pos_px) = (self.code_px(z), 12.5 * z);
        let (num_right, caret_w) = (gutter - 12. * z, (1.5 * z).max(1.));
        let glyphs = self.ascii_glyphs.clone();
        layer = layer.child(canvas(|_, _, _| (), move |b: Bounds<Pixels>, _, window, _| {
            let base = font(mono());
            let italic = base.clone().italic();
            let ts = window.text_system().clone();
            let lh = px(lh);
            for r in rows {
                let origin = point(b.origin.x + px(r.dx), b.origin.y + px(r.y));
                for bg in r.bg.into_iter().flatten() { window.paint_quad(fill(Bounds::new(origin, size(b.size.width, lh)), bg)); }
                if let Some((x, w)) = r.sel { window.paint_quad(fill(Bounds::new(point(origin.x + px(x), origin.y), size(px(w), lh)), cola(SEL, 0.6))); }
                for (x, w, current) in &r.found {
                    let b = Bounds::new(point(origin.x + px(*x), origin.y), size(px(*w), lh));
                    window.paint_quad(if *current { quad(b, px(2.), col(0x515c6a), px(1.), col(WARN), BorderStyle::Solid) } else { quad(b, px(2.), cola(0xea5c00, 0.33), px(0.), transparent_black(), BorderStyle::Solid) });
                }
                if r.covered { continue; }
                let mut g = glyphs.borrow_mut();
                paint_mono(&r.num, &[], cola(MUTED, 0.7 * r.alpha), 1., point(origin.x + px(num_right - r.num.len() as f32 * cw), origin.y), lh, font_px, pos_px, cw, &mut g, &base, &italic, window);
                if r.text.is_ascii() {
                    paint_mono(&r.text, &r.hl, col(FG), r.alpha, point(origin.x + px(gutter), origin.y), lh, font_px, pos_px, cw, &mut g, &base, &italic, window);
                } else {
                    // other scripts: shaped, so fallback fonts and wide characters are placed properly
                    let code = ts.layout_line(&r.text, px(SHAPE_PX), &text_runs(r.text.len(), &r.hl, &base, &italic), None);
                    paint_scaled(&code, &r.hl, col(FG), r.alpha, point(origin.x + px(gutter), origin.y), lh, font_px, pos_px, window);
                }
                if let Some(x) = r.caret { window.paint_quad(fill(Bounds::new(point(origin.x + px(x), origin.y), size(px(caret_w), lh)), col(WARN))); }
            }
            let sf = window.scale_factor();
            for (img, x, y, w, h, top, bottom) in tile_draws {
                // Edges on whole device pixels: neighbouring cells meet without seams, and a cell drawn
                // at its own zoom maps pixel for pixel, as sharp as direct text. Each is clipped to the
                // lines it was picked for.
                let d = |v: f32| (v * sf).round() / sf;
                let (x0, y0) = (d(fl(b.origin.x) + x), d(fl(b.origin.y) + y));
                let (x1, y1) = (d(fl(b.origin.x) + x + w), d(fl(b.origin.y) + y + h));
                let (c0, c1) = (d(fl(b.origin.y) + top), d(fl(b.origin.y) + bottom));
                let clip = gpui::ContentMask { bounds: Bounds::new(point(b.origin.x, px(c0)), size(b.size.width, px(c1 - c0))) };
                window.with_content_mask(Some(clip), |window| { let _ = window.paint_image(Bounds::new(point(px(x0), px(y0)), size(px(x1 - x0), px(y1 - y0))), gpui::Corners::default(), img, 0, false); });
            }
        }).absolute().size_full());
        self.live = live;
        layer
    }

    /// The text cells for lines `first..last` of a card, `x_range` (world units across the card) in view.
    /// Returns which of those lines the cells cover (one flag per line from `first`), and what to paint:
    /// (image, x, y, width, height, clip top, clip bottom) in logical pixels from the top-left of the
    /// card's text layer. Each run of lines is drawn from the best zoom level whose cells for it are
    /// ready, clipped to those lines, so levels never overlap; missing cells are asked of the worker
    /// threads, those for lines nothing covers yet first. Lines no level covers are drawn directly.
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn cells_for(&mut self, pane: u64, card: &Card, spans: &Rc<Spans>, start_no: usize, first: usize, last: usize, x_range: (f32, f32), ahead: (Range<usize>, (f32, f32)), z: f32) -> (Vec<bool>, Vec<(Arc<gpui::RenderImage>, f32, f32, f32, f32, f32, f32)>) {
        let (id, sf) = (card.id, self.scale);
        let raw = self.views.get(&pane).copied().unwrap_or(self.views[&self.active]);
        // While gliding, ask for the zoom step the glide will reach in about two frames; at rest, the exact zoom.
        let gliding = (raw.t.z - raw.v.z).abs() > 1e-4;
        let want = if gliding { tiles::step_up(z * (raw.t.z / z).powf(0.4)) } else { z };
        let n = card.lines.len();
        let (gutter, cw) = (self.gutter(n), self.char_w(1.));
        let (ch, cwd) = (tiles::CELL_H as f32, tiles::CELL_W as f32);
        let span_cols = |l: f32, xr: (f32, f32)| {
            let k = l * sf;
            let max_col = (((card.w * k) / cwd).ceil() as u32).max(1) - 1;
            ((xr.0 * k / cwd).floor().max(0.) as u32).min(max_col)..=(((xr.1 * k - 0.01) / cwd).floor().max(0.) as u32).min(max_col)
        };
        let cols_at = |l: f32| span_cols(l, x_range);
        // the cell rows line `i` touches at zoom `l`
        let rows_of = |l: f32, i: usize| { let k = l * sf; ((i as f32 * LH * k / ch) as u32)..=((((i + 1) as f32 * LH * k) - 0.01) / ch) as u32 };
        let content_ok = |c: &tiles::Cell| Arc::ptr_eq(&c.lines, &card.lines) && c.start_no == start_no && c.scale == sf;
        // Levels to draw from, best first: the wanted one, the last complete one, then whatever else this
        // card has cells at, nearest zoom first (shrinking preferred to enlarging). At rest a level that
        // would be enlarged by more than 12% is left out: its lines are drawn directly for the frame or
        // two before the exact cells arrive.
        let shown = self.tile_shown.get(&(pane, id)).copied();
        let mut levels: Vec<f32> = vec![want];
        levels.extend(shown);
        let mut others: Vec<f32> = self.tile_levels.get(&(pane, id)).cloned().unwrap_or_default();
        others.sort_by(|a, b| { let d = |l: f32| (l / z).ln().abs() + if l < z { 0.5 } else { 0. }; d(*a).total_cmp(&d(*b)) });
        levels.extend(others);
        levels.dedup_by(|a, b| a.to_bits() == b.to_bits());
        let mut seen = HashSet::new();
        levels.retain(|l| seen.insert(l.to_bits()) && (gliding || l * 1.13 >= z));
        // which (level, cell row) are ready across every column in view
        let mut row_ready: HashMap<(u32, u32), bool> = HashMap::new();
        let mut ready = |me: &Self, l: f32, cy: u32| *row_ready.entry((l.to_bits(), cy)).or_insert_with(|| {
            cols_at(l).all(|cx| me.tiles.get(&(pane, id, l.to_bits(), cx, cy)).is_some_and(|c| content_ok(c) && c.img.is_some()))
        });
        // each line's level (index into `levels`), or none
        let mut pick: Vec<Option<usize>> = Vec::with_capacity(last - first);
        for i in first..last {
            pick.push(levels.iter().position(|&l| rows_of(l, i).all(|cy| ready(self, l, cy))));
        }
        // Ask for missing cells at `want`: first rows of lines nothing covers, then rows that will come
        // into view where the view is heading, then the rest. (priority, row, columns)
        let mut rows: Vec<(u8, u32, std::ops::RangeInclusive<u32>)> = Vec::new();
        for (j, i) in (first..last).enumerate() {
            let p = if pick[j].is_none() { 0 } else { 2 };
            for cy in rows_of(want, i) { match rows.last_mut() { Some(r) if r.1 == cy => r.0 = r.0.min(p), _ => rows.push((p, cy, cols_at(want))) } }
        }
        let (ahead_lines, ahead_x) = ahead;
        let wide = { let (a, b) = (span_cols(want, ahead_x), cols_at(want)); (*a.start()).min(*b.start())..=(*a.end()).max(*b.end()) };
        for i in ahead_lines {
            for cy in rows_of(want, i) {
                match rows.iter_mut().find(|r| r.1 == cy) { Some(r) => r.2 = wide.clone(), None => rows.push((1, cy, wide.clone())) }
            }
        }
        rows.sort_by_key(|r| r.0);
        rows.truncate(64);
        'ask: for (_, cy, cols) in rows {
            for cx in cols {
                let key = (pane, id, want.to_bits(), cx, cy);
                if let Some(c) = self.tiles.get_mut(&key) { if content_ok(c) { c.used = self.frame; continue; } }
                if !self.tile_pool.room() { self.fx.want_frame = true; break 'ask; }
                if let Some(old) = self.tiles.remove(&key) { self.dead_tiles.extend(old.img); }
                let k = want * sf;
                let a = (((cy * tiles::CELL_H) as f32 / (LH * k)).floor() as usize).saturating_sub(1).min(n);
                let b = (((((cy + 1) * tiles::CELL_H) as f32 / (LH * k)).ceil() as usize) + 1).min(n);
                self.tiles.insert(key, tiles::Cell { lines: card.lines.clone(), start_no, scale: sf, img: None, missing: false, asked: true, used: self.frame });
                self.tile_pool.send(tiles::Job { key, lines: card.lines.clone(), range: a..b, spans: spans.slice(a, b), start_no, z: want, scale: sf, gutter, num_right: gutter - 12., cw, lh: LH });
            }
        }
        // draw each run of lines from its level, clipped to those lines
        let mut draws = Vec::new();
        let mut j = 0;
        while j < pick.len() {
            let Some(li) = pick[j] else { j += 1; continue };
            let mut e = j;
            while e + 1 < pick.len() && pick[e + 1] == Some(li) { e += 1; }
            let l = levels[li];
            let per = z / (l * sf); // logical pixels per device pixel of the cell
            let (top, bottom) = (PAD * z + (first + j) as f32 * LH * z, PAD * z + (first + e + 1) as f32 * LH * z);
            let (r0, r1) = (*rows_of(l, first + j).start(), *rows_of(l, first + e).end());
            for cy in r0..=r1 { for cx in cols_at(l) {
                if let Some(c) = self.tiles.get_mut(&(pane, id, l.to_bits(), cx, cy)) {
                    c.used = self.frame;
                    if let Some(img) = &c.img { draws.push((img.clone(), (cx * tiles::CELL_W) as f32 * per, PAD * z + (cy * tiles::CELL_H) as f32 * per, cwd * per, ch * per, top, bottom)); }
                }
            } }
            j = e + 1;
        }
        let complete = pick.iter().all(|p| *p == Some(0));
        if complete { self.tile_shown.insert((pane, id), want); }
        if !complete || self.tile_pool.in_flight > 0 { self.fx.want_frame = true; }
        (pick.iter().map(Option::is_some).collect(), draws)
    }

    /// The region bars, highlights and resize handles inside a file card.
    fn regions_el(&mut self, card: &Card, pane: u64, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.editing == Some(card.id) { return Vec::new(); }
        let z = self.pv(pane).v.z;
        let mut out = Vec::new();
        for r in self.doc.marks_of(card.id) {
            let gone = self.live.as_ref().is_some_and(|l| !l.ref_mode && l.src.is_some_and(|s| s.0 == card.id && match l.region { Some(id) => id == r.id, None => r.start >= s.1 && r.end <= s.2 }));
            if gone { continue; }
            let y = (HEAD + PAD + r.start as f32 * LH + self.line_shift(card.id, r.start)) * z;
            let h = (r.end - r.start + 1) as f32 * LH * z;
            let id = r.id;
            let fade = self.fx.fresh_alpha(id);
            if fade <= 0.01 { continue; }
            let selected = self.selected.contains(&id);
            let tint = self.hover_tab == Some(id) || (self.picked == Some(id) && selected);
            let refd = self.doc.is_referenced(id) || self.live.as_ref().is_some_and(|l| l.ref_mode && l.region == Some(id));
            let doomed = self.doomed.contains(&id);
            let mut ov = div().absolute().left_0().top(px(y)).w_full().h(px(h)).border_t_1().border_b_1().border_color(transparent_black()).when(fade < 1., |d| d.opacity(fade));
            if tint { ov = ov.bg(cola(r.color, 0.10)).border_color(cola(r.color, 0.35)); }
            if refd { ov = ov.bg(cola(CARD, 0.62)).border_dashed().border_color(cola(r.color, 0.6)); }
            if doomed { ov = ov.bg(cola(BAD, 0.14)); }
            out.push(ov.into_any_element());
            let edge = (6. * z).max(4.);
            let bar_color = if doomed { BAD } else { r.color };
            let rest = if doomed { 0.5 } else if refd { 0.45 } else if selected { 1. } else { 0.75 };
            let group: SharedString = format!("bar{id}").into();
            let wide = px(9. * z);
            out.push(div().group(group.clone()).absolute().left_0().top(px(y)).h(px(h)).w(px((16. * z).max(8.))).cursor_grab().when(fade < 1., |d| d.opacity(fade))
                .child(div().absolute().left_0().top_0().h_full().w(px(if selected { 9. } else { 5. } * z)).rounded_r(px(3. * z)).bg(cola(bar_color, rest)).group_hover(group, move |s| s.w(wide).bg(col(bar_color))))
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                    if t.space { cx.stop_propagation(); let f = t.doc.region(id).map(|r| r.file).unwrap_or(0); t.start_move(f, e, pane, cx); return; }
                    t.start_region(id, Via::Bar, e, pane, cx);
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |t, e: &MouseDownEvent, _, cx| { cx.stop_propagation(); t.region_menu(id, fl(e.position.x), fl(e.position.y)); cx.notify(); }))
                .child(div().absolute().left_0().top_0().w_full().h(px(edge)).cursor_ns_resize().rounded(px(2.)).hover(move |s| s.bg(cola(bar_color, 0.7)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| t.start_edge(id, true, e, cx))))
                .child(div().absolute().left_0().bottom_0().w_full().h(px(edge)).cursor_ns_resize().rounded(px(2.)).hover(move |s| s.bg(cola(bar_color, 0.7)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| t.start_edge(id, false, e, cx))))
                .into_any_element());
        }
        out
    }

    fn header_el(&mut self, card: &Card, pane: u64, z: f32, cx: &mut Context<Self>) -> Div {
        let id = card.id;
        let is_file = card.is_file();
        let in_ctx = self.doc.ctx.contains(&id);
        let selected = self.selected.contains(&id);
        let editing = self.editing == Some(id);
        let group: SharedString = format!("card{id}").into();
        let mut tools = div().flex().flex_none().gap(px(z)).when(!selected && !editing, |d| d.opacity(0.55).group_hover(group, |s| s.opacity(1.)));
        let sz = 14. * z;
        let ctx_color = if in_ctx { OK } else { MUTED };
        if is_file {
            let path = card.path.clone().unwrap();
            let explain = if self.explaining.contains_key(&path) {
                self.fx.want_frame = true;
                let turn = (ms_since(self.started) / 1100.).fract() * std::f32::consts::TAU;
                icon("explain", sz, col(SNIP)).with_transformation(Transformation::rotate(radians(turn)))
            } else { icon("explain", sz, col(MUTED)) };
            tools = tools
                .child(tool_with(explain, sz, cx, move |t, _, _| t.explain(path.clone())))
                .child(tool("search", sz, MUTED, cx, move |t, _, _| if t.finds.contains_key(&id) { t.close_find(id) } else { t.open_find(id, false) }))
                .child(tool("notes", sz, MUTED, cx, move |t, _, _| t.toggle_notes(id)))
                .child(tool("chat", sz, MUTED, cx, move |t, _, _| t.toggle_chat(id)))
                .child(tool("ctx", sz, ctx_color, cx, move |t, _, _| t.toggle_ctx(id)))
                .child(tool("save", sz, MUTED, cx, move |t, _, _| t.save_file(id)))
                .child(tool("editor", sz, MUTED, cx, move |t, _, _| t.open_in_editor(id, 1)))
                .child(tool("close", sz, MUTED, cx, move |t, _, _| t.close_card(id)));
        } else {
            let is_ref = card.ref_of.is_some();
            if let Some(r) = card.ref_of { tools = tools.child(tool("editor", sz, MUTED, cx, move |t, _, _| t.focus_card(r))); }
            if card.target.is_some() { tools = tools.child(tool("merge", sz, MUTED, cx, move |t, _, _| t.apply_to_target(id))); }
            tools = tools
                .child(tool("search", sz, MUTED, cx, move |t, _, _| if t.finds.contains_key(&id) { t.close_find(id) } else { t.open_find(id, false) }))
                .child(tool("ctx", sz, ctx_color, cx, move |t, _, _| t.toggle_ctx(id)))
                .child(tool("chat", sz, MUTED, cx, move |t, _, _| t.toggle_chat(id)))
                .child(tool(if is_ref { "close" } else { "trash" }, sz, MUTED, cx, move |t, _, _| t.close_card(id)));
        }
        let sub = if let Some(r) = card.ref_of.and_then(|r| self.doc.region(r)) {
            format!("↗ {} lines {}–{}", self.doc.card(r.file).and_then(|f| f.path.clone()).unwrap_or_default(), r.start + 1, r.end + 1)
        } else if let Some(t) = &card.target {
            let wh = t.replace.map(|(a, b)| format!("replace {a}–{b}")).or(t.insert_at.map(|n| format!("near line {n}"))).unwrap_or_default();
            format!("→ {} {wh}", t.file)
        } else { String::new() };
        let renaming = self.focus_input == Some(Input::Rename(id));
        let title: AnyElement = if renaming {
            div().flex_1().px(px(4. * z)).bg(col(INPUT)).border_1().border_color(col(ACCENT)).rounded(px(4. * z)).child(self.input_el(Input::Rename(id), "", 12.5 * z, cx)).into_any_element()
        } else if is_file {
            let (dir, base) = dir_and_base(&card.title);
            div().flex_1().min_w_0().flex().overflow_hidden().whitespace_nowrap()
                .child(div().text_color(col(MUTED)).child(dir.to_string())).child(div().font_weight(FontWeight::MEDIUM).child(base.to_string())).into_any_element()
        } else {
            div().flex_1().min_w_0().flex().gap(px(8. * z)).overflow_hidden().whitespace_nowrap()
                .child(div().font_weight(FontWeight::MEDIUM).child(card.title.clone())).child(div().text_color(col(MUTED)).text_size(tz(11., z)).font_family(mono()).child(sub)).into_any_element()
        };
        let ico = if is_file { div().bg(col(ext_color(&card.title))).rounded(px(2. * z)) }
            else if card.ref_of.is_some() { div().border_2().border_color(col(card.color)).rounded_full() }
            else { div().bg(col(card.color)).rounded_full() };
        let bg = if is_file { col(HEADC) } else { mix(card.color, HEADC, 0.13) };
        div().relative().flex().items_center().gap(px(7. * z)).h(px(HEAD * z)).pl(px(11. * z)).pr(px(6. * z)).bg(bg).rounded_t(px((9. * z - 1.).max(0.))).border_b_1().border_color(col(BORDER))
            .text_size(tz(12.5, z)).text_color(col(FG)).cursor_grab()
            .child(ico.size(px(9. * z)).flex_none()).child(title)
            .when(card.dirty(), |d| d.child(div().size(px(8. * z)).flex_none().rounded_full().bg(col(FG))))
            .when(editing, |d| d.child(div().flex_none().text_size(tz(10., z)).text_color(col(WARN)).whitespace_nowrap().child("EDITING · Esc to finish · Ctrl+S to save")))
            .child(tools)
            .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                if e.click_count >= 2 && t.doc.card(id).is_some_and(|c| !c.is_file()) { cx.stop_propagation(); t.begin_rename(id); cx.notify(); return; }
                t.start_move(id, e, pane, cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |t, e: &MouseDownEvent, _, cx| { cx.stop_propagation(); t.card_menu(id, pane, fl(e.position.x), fl(e.position.y)); cx.notify(); }))
    }

    fn card_box(&self, x: f32, y: f32, w: f32, h: f32, z: f32, border: Hsla) -> Div {
        div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).bg(col(CARD)).border_1().border_color(border).rounded(px(9. * z)).overflow_hidden()
    }

    fn render_cards(&mut self, pane: u64, vw: f32, vh: f32, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let v = self.pv(pane).v;
        let z = v.z;
        let (over, ghost_card) = self.live.as_ref().map_or((None, None), |l| (l.over, l.ghost_card));
        let live_pane_ok = self.live.as_ref().is_some_and(|l| self.shared || l.pane == pane);
        let moving: Vec<u64> = match &self.drag { Some(Drag::Move { items, moved: true, .. }) => items.iter().map(|i| i.id).collect(), _ => Vec::new() };
        let mut out: Vec<AnyElement> = Vec::new();
        let mut drawers: Vec<AnyElement> = Vec::new(); // tabs and chats always sit above every card
        for i in 0..self.doc.cards.len() {
            // Decide from the card's frame whether it is on screen before copying any of its text.
            let src = &self.doc.cards[i];
            if !self.in_pane(src, pane) { continue; }
            let n_lines = src.lines.len();
            let roomy = n_lines + self.live.as_ref().map_or(0, |l| l.n) + 8;
            let (rx, ry, rh) = (src.x * z + v.x, src.y * z + v.y, (HEAD + PAD * 2. + roomy as f32 * LH) * z);
            let settled = !self.fx.slides.contains_key(&src.id) && self.editing != Some(src.id);
            if settled && (ry > vh + 2100. * z || ry + rh < -700. || rx > vw + 1000. || rx + src.w * z + 1000. * z < 0.) { continue; }
            let c = &if z < FAR_Z {
                Card { id: src.id, path: src.path.clone(), title: src.title.clone(), lang: String::new(), pane: src.pane, x: src.x, y: src.y, w: src.w, lines: Default::default(), disk: Default::default(), color: src.color, talk: Talk::default(), ref_of: src.ref_of, target: None, sel: None }
            } else { src.clone() };
            if ghost_card == Some(c.id) && over.is_some() { continue; } // drawn inside its slot
            let (alpha, rise) = self.fx.card(c.id);
            if alpha <= 0. { continue; } // its lines are still flying in
            let (live_n, extra) = self.live.as_ref().map_or((0, 0.), |l| {
                let lifted = l.src.is_some_and(|s| s.0 == c.id) && !l.ref_mode;
                let gap = l.over == Some(c.id);
                ((if gap { l.n as isize } else { 0 }) - (if lifted { l.n as isize } else { 0 }), if gap { 2. * GP } else { 0. })
            });
            let editing_n = (self.editing == Some(c.id)).then(|| self.inputs[&Input::Editor(c.id)].lines.len());
            let n = (editing_n.unwrap_or(n_lines) as isize + live_n).max(1) as f32;
            let h_world = (HEAD + PAD * 2. + n * LH + extra + self.fx.extra(c.id)).max(HEAD + PAD * 2.);
            let slide = self.fx.slide(c.id);
            // an undocked card being dragged eases back from the slot it just left
            let unsnap = if ghost_card == Some(c.id) { self.fx.tween(Tw::Ghost, 0., 120.) } else { 0. };
            // cards start on whole pixels, so their rows of text do too whenever the line pitch is whole
            let (sx, sy, sw, sh) = (((c.x + slide.0) * z + v.x).round(), ((c.y + slide.1 + rise + unsnap) * z + v.y).round(), c.w * z, h_world * z);
            if sy > vh + 2000. * z || sy + sh < -600. || sx > vw + 900. || sx + sw + 900. * z < 0. { continue; }
            let selected = self.selected.contains(&c.id);
            let editing = self.editing == Some(c.id);
            let border = if editing { col(WARN) } else if selected { col(ACCENT) } else if c.is_file() { col(BORDER) } else { mix(c.color, BORDER, 0.55) };
            if z < FAR_Z {
                let (dir, base) = dir_and_base(&c.title);
                let name = if c.ref_of.is_some() { format!("↗ {}", c.title) } else { base.to_string() };
                let small = if c.is_file() { format!("{dir} · {n_lines} lines") } else { format!("{n_lines} lines") };
                out.push(self.card_box(sx, sy, sw, sh, z, border).bg(cola(CARD, 0.92)).flex().flex_col().items_center().justify_center().gap(px(2.)).px(px(6.))
                    .child(div().text_size(tpx(15.)).font_weight(FontWeight::SEMIBOLD).text_color(col(if c.is_file() { FG } else { c.color })).whitespace_nowrap().child(name))
                    .when(sh > 44., |d| d.child(div().text_size(tpx(11.)).text_color(col(MUTED)).whitespace_nowrap().child(small))).into_any_element());
                continue;
            }
            let id = c.id;
            let docking = over == Some(id) && live_pane_ok;
            let lines = self.lines_el(c, pane, sx, sy, vw, vh, docking, cx);
            let regions = self.regions_el(c, pane, cx);
            let header = self.header_el(c, pane, z, cx);
            let resize = div().absolute().right_0().bottom_0().size(px(14. * z)).cursor_nwse_resize()
                .child(div().absolute().right(px(4. * z)).bottom(px(4. * z)).size(px(6. * z)).border_r_2().border_b_2().border_color(cola(MUTED, 0.5)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if let Some(c) = t.doc.card(id) { let w = c.w; t.drag = Some(Drag::Resize { id, sx: fl(e.position.x), ow: w, z }); }
                }));
            let shadows = if editing { vec![ring(col(WARN)), shadow(10. * z, 30. * z, 0.47)] }
                else if selected { vec![ring(col(ACCENT)), shadow(10. * z, 30. * z, 0.47)] }
                else if moving.contains(&id) { vec![shadow(18. * z, 48. * z, 0.67)] }
                else { vec![shadow(6. * z, 22. * z, 0.33)] };
            let group: SharedString = format!("card{id}").into();
            let shell = self.card_box(sx, sy, sw, sh, z, border).group(group.clone()).shadow(shadows).when(c.ref_of.is_some(), |d| d.border_dashed()).when(alpha < 1., |d| d.opacity(alpha));
            if docking {
                // the file's lines glide over the dragged block, which sits in the gap beneath them
                let lines_overlay = div().group(group).absolute().left(px(sx)).top(px(sy)).w(px(sw)).h(px(sh)).overflow_hidden().child(lines).children(regions).child(header);
                out.push(shell.into_any_element());
                out.push(self.ghost_el(v, true).into_any_element());
                out.push(lines_overlay.into_any_element());
            } else {
                let bar = self.find_bar(id, c.w, z, cx);
                out.push(shell.child(lines).children(regions).child(header).child(resize).children(bar).into_any_element());
            }
            drawers.extend(self.overlays(c, pane, sx, sy, sw, z, cx));
        }
        if live_pane_ok && over.is_none() && self.live.as_ref().is_some_and(|l| l.ghost_card.is_none()) {
            out.push(self.ghost_el(v, false).into_any_element());
        }
        out.extend(drawers);
        out
    }

    /// The block being dragged. Over a file it snaps (with a short glide) into the padded slot, header hidden,
    /// while still following the pointer sideways.
    fn ghost_el(&mut self, v: View, docked: bool) -> Div {
        let l = self.live.as_ref().unwrap();
        let z = v.z;
        let (mut x, mut free_y) = (l.gx, l.gy - HEAD - PAD);
        if let Some(c) = l.ghost_card.and_then(|i| self.doc.card(i)) { x = c.x; free_y = c.y; }
        let slot = l.over.and_then(|f| self.doc.card(f)).filter(|_| docked).map(|f| (f.y + l.at as f32 * LH + GP, f.lines.len() + l.n));
        let lines = l.lines.clone();
        let (lang, title, color, n, gw, at, is_sel, ref_mode) = (l.lang.clone(), l.title.clone(), l.color, l.n, l.gw, l.at, l.title.is_empty(), l.ref_mode);
        let snap = self.fx.tween(Tw::Ghost, slot.map_or(0., |s| s.0 - free_y), 120.);
        let y = free_y + snap;
        let id = u64::MAX;
        self.set_spans(id, &lines, &lang);
        let spans = self.spans[&id].1.clone();
        let gutter = self.gutter(slot.map_or(lines.len().max(100), |s| s.1)) * z;
        let lh = LH * z;
        let mut body = div().absolute().left_0().top(px(HEAD * z)).w_full().font_family(mono()).text_size(tz(12.5, z)).line_height(px(lh));
        for (i, line) in lines.iter().enumerate().take(400) {
            let (text, hl) = display_line(line, spans.line(i));
            let no = if slot.is_some() { at + i + 1 } else { i + 1 };
            body = body.child(div().absolute().left_0().w_full().top(px((PAD + i as f32 * LH) * z)).h(px(lh)).flex().when(is_sel, |d| d.bg(mix(SEL, CARD, 0.55)))
                .child(div().w(px(gutter)).flex_none().flex().justify_end().pr(px(12. * z)).text_color(cola(MUTED, 0.7)).child(no.to_string()))
                .child(div().flex_1().text_color(col(FG)).whitespace_nowrap().overflow_hidden().child(StyledText::new(text).with_highlights(hl))));
        }
        let h = (HEAD + PAD * 2. + n as f32 * LH) * z;
        let head = div().h(px(HEAD * z)).pl(px(11. * z)).flex().items_center().gap(px(7. * z)).text_size(tz(12.5, z)).text_color(col(FG)).bg(mix(color, HEADC, 0.13)).border_b_1().border_color(col(BORDER))
            .when(docked, |d| d.opacity(0.))
            .child(div().size(px(9. * z)).flex_none().rounded_full().bg(col(color))).child(div().font_weight(FontWeight::MEDIUM).child(if is_sel { format!("{n} line{}", if n == 1 { "" } else { "s" }) } else { title }));
        let border = if is_sel { col(ACCENT) } else { mix(color, BORDER, 0.55) };
        let shadows = if docked { vec![ring(cola(color, 0.5))] } else { vec![shadow(18. * z, 50. * z, 0.73)] };
        div().absolute().left(px(x * z + v.x)).top(px(y * z + v.y)).w(px(gw * z)).h(px(h)).bg(col(CARD)).border_1().border_color(border).when(ref_mode, |d| d.border_dashed()).rounded(px(9. * z)).shadow(shadows)
            .overflow_hidden().child(head).child(body)
    }

    /// The tabs and drawers beside a card.
    fn overlays(&mut self, c: &Card, pane: u64, sx: f32, sy: f32, sw: f32, z: f32, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let x = sx + sw;
        if c.is_file() {
            self.chat_stack(c.id, "Chat".to_string(), ACCENT, false, x, sy + 6. * z, pane, z, cx, &mut out);
            let mut last = 30.;
            if self.editing != Some(c.id) {
                for r in self.doc.marks_of(c.id) {
                    let gone = self.live.as_ref().is_some_and(|l| l.region == Some(r.id) && !l.ref_mode);
                    if gone { continue; }
                    let top = f32::max(HEAD + PAD + r.start as f32 * LH + self.line_shift(c.id, r.start), last + 2.);
                    last = top + TAB_GAP + 22.;
                    self.chat_stack(r.id, r.title.clone(), r.color, true, x, sy + top * z, pane, z, cx, &mut out);
                }
            }
        } else {
            self.chat_stack(c.id, c.title.clone(), c.color, true, x, sy + 8. * z, pane, z, cx, &mut out);
        }
        out
    }

    /// A margin tab: arrow (turns as its drawer opens), label, and whatever else is passed in.
    fn tab_el(&self, color: u32, open: f32, z: f32) -> Div {
        div().flex().flex_none().items_center().gap(px(5. * z)).h(px(22. * z)).max_w(px(220. * z)).pl(px(6. * z)).pr(px(9. * z)).bg(col(HEADC)).border_1().border_l_0().border_color(col(BORDER)).rounded_r(px(7. * z))
            .text_size(tz(11.5, z)).text_color(col(MUTED)).whitespace_nowrap().overflow_hidden().cursor_pointer().hover(|s| s.bg(col(HOVER)).text_color(col(FG)))
            .child(icon("tri", 8. * z, col(color)).with_transformation(Transformation::rotate(radians(open * std::f32::consts::PI))))
    }

    /// One owner's column beside its card: the chat tab with its drawer, then the draft tab with its drawer.
    #[allow(clippy::too_many_arguments)]
    fn chat_stack(&mut self, id: u64, label: String, color: u32, has_draft: bool, x: f32, y: f32, pane: u64, z: f32, cx: &mut Context<Self>, out: &mut Vec<AnyElement>) {
        let Some(talk) = self.doc.talk(id) else { return };
        let (talk_open, draft_open, has_chat) = (talk.open, talk.draft_open, talk.chat.iter().any(|m| m.role != Role::Note));
        let fade = self.fx.fresh_alpha(id);
        if fade <= 0.01 { return; }
        let is_region = self.doc.region(id).is_some();
        let is_file = self.doc.card(id).is_some_and(Card::is_file);
        let open = self.fx.tween(Tw::Drawer(id), if talk_open { 1. } else { 0. }, 280.);
        let renaming = is_region && self.focus_input == Some(Input::Rename(id));
        let mut chat_tab = self.tab_el(color, open, z).when(is_region, |d| d.cursor_grab());
        chat_tab = if renaming {
            chat_tab.child(div().w(px(150. * z)).px(px(3. * z)).bg(col(INPUT)).border_1().border_color(col(ACCENT)).rounded(px(4. * z)).child(self.input_el(Input::Rename(id), "", 11.5 * z, cx)))
        } else {
            chat_tab.child(div().overflow_hidden().when(is_file, |d| d.font_weight(FontWeight::MEDIUM)).child(label))
        };
        let chat_tab = chat_tab.when(has_chat, |d| d.child(div().size(px(5. * z)).flex_none().rounded_full().bg(col(color))))
            .on_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                if matches!(t.focus_input, Some(Input::Rename(r)) if r == id) { return; }
                if is_region && !t.space { t.start_region(id, Via::Tab, e, pane, cx); } else { t.toggle_chat(id); cx.notify(); }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |t, e: &MouseDownEvent, _, cx| { if t.doc.region(id).is_some() { t.region_menu(id, fl(e.position.x), fl(e.position.y)); } cx.notify(); }))
            .id(("tab", id)).on_hover(cx.listener(move |t, h: &bool, _, cx| { if t.doc.region(id).is_some() { t.hover_tab = h.then_some(id); cx.notify(); } }));
        let mut chat_row = div().flex().items_start().child(chat_tab);
        if open > 0.01 {
            let panel = self.chat_panel(id, 360. * z, 440. * z, z, false, cx);
            chat_row = chat_row.child(div().flex_none().w(px(360. * z * open)).overflow_hidden().opacity((open * 1.5).min(1.)).child(panel));
        }
        let mut stack = div().absolute().left(px(x)).top(px(y)).flex().flex_col().items_start().gap(px(2. * z)).when(fade < 1., |d| d.opacity(fade)).child(chat_row);
        if has_draft {
            let dopen = self.fx.tween(Tw::Draft(id), if draft_open { 1. } else { 0. }, 280.);
            let stats = self.doc.draft_stats(id).filter(|(a, d)| *a + *d > 0);
            let dt = self.tab_el(color, dopen, z).text_size(tz(11., z)).when(stats.is_some(), |d| d.text_color(col(FG))).child("✎ Draft")
                .when_some(stats, |d, (a, del)| d.child(div().flex().gap(px(4. * z)).font_family(mono()).text_size(tz(10.5, z)).font_weight(FontWeight::SEMIBOLD)
                    .child(div().text_color(col(OK)).child(format!("+{a}"))).child(div().text_color(col(BAD)).child(format!("−{del}")))))
                .on_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { t.toggle_draft(id); cx.notify(); }));
            let mut draft_row = div().flex().items_start().child(dt);
            if dopen > 0.01 {
                let d = self.draft_panel(id, 460. * z, z, cx);
                draft_row = draft_row.child(div().flex_none().w(px(460. * z * dopen)).overflow_hidden().opacity((dopen * 1.5).min(1.)).child(d));
            }
            stack = stack.child(draft_row);
        }
        out.push(stack.into_any_element());
    }

    fn selection_tools(&mut self, pane: u64, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.drag.is_some() || self.editing.is_some() { return None; }
        let v = self.pv(pane).v;
        let c = self.doc.cards.iter().find(|c| c.sel.is_some() && self.in_pane(c, pane))?.clone();
        let (a, b) = c.sel_range()?;
        let id = c.id;
        let p = ease_out(ms_since(self.sel_at) / 150.);
        if p < 1. { self.fx.want_frame = true; }
        let (x, y) = (c.x * v.z + v.x + 40., (c.y + HEAD + PAD + (b + 1) as f32 * LH) * v.z + v.y + 6. + 4. * (1. - p));
        let spot = (c.x + c.w + 80., c.y + HEAD + PAD + a as f32 * LH - 40.);
        let btn = |label: &'static str, cx: &mut Context<Self>, f: Act| -> Div {
            div().px(px(9.)).py(px(4.)).rounded(px(5.)).cursor_pointer().hover(|s| s.bg(col(HOVER))).whitespace_nowrap().child(label)
                .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, w, cx| { cx.stop_propagation(); f(t, w, cx); cx.notify(); }))
        };
        let pane_id = c.pane;
        Some(div().absolute().left(px(x)).top(px(y)).flex().gap(px(2.)).p(px(3.)).bg(col(CARD)).border_1().border_color(col(BORDER)).rounded(px(7.)).shadow(vec![shadow(8., 24., 0.47)]).text_size(tpx(12.)).text_color(col(FG)).opacity(p)
            .child(btn("◆ Snippet", cx, Rc::new(move |t, _, _| { t.make_selection_region(id); })))
            .child(btn("✂ Detach", cx, Rc::new(move |t, _, _| { if let Some(r) = t.make_selection_region(id) { t.detach_fx(r, spot.0, spot.1, pane_id); t.touch(); } })))
            .child(btn("↗ Reference", cx, Rc::new(move |t, _, _| { if let Some(r) = t.make_selection_region(id) { t.reference_fx(r, spot.0, spot.1, pane_id); t.touch(); } })))
            .child(btn("⧉ Copy out", cx, Rc::new(move |t, _, _| { t.snapshot(); t.copy_fx(id, a, b, spot.0, spot.1); if let Some(c) = t.doc.card_mut(id) { c.sel = None; } t.touch(); })))
            .child(btn("? Ask", cx, Rc::new(move |t, _, _| { if let Some(r) = t.make_selection_region(id) { t.toggle_chat(r); t.focus_input = Some(Input::Chat(r)); } })))
            .child(btn("◎ Context", cx, Rc::new(move |t, _, _| { if let Some(r) = t.make_selection_region(id) { t.toggle_ctx(r); } })))
            .into_any_element())
    }

    /// Dashed connectors from a drafted snippet to the line it was suggested for (pane coordinates).
    fn links(&self, pane: u64, v: View) -> Vec<([f32; 8], u32)> {
        let mut out = Vec::new();
        for s in self.doc.cards.iter().filter(|c| !c.is_file() && self.in_pane(c, pane)) {
            let Some(t) = &s.target else { continue };
            let Some(f) = self.doc.file_by_path(&t.file).filter(|f| self.in_pane(f, pane)) else { continue };
            let line = t.replace.map(|r| r.0).or(t.insert_at).unwrap_or(1).saturating_sub(1);
            let (x0, y0) = (f.x + f.w, f.y + HEAD + PAD + line as f32 * LH + LH / 2.);
            let to_left = s.x > x0;
            let (x1, y1) = (if to_left { s.x } else { s.x + s.w }, s.y + 17.);
            let bend = ((x1 - x0).abs() / 2.).max(60.);
            let w = [x0, y0, x0 + bend, y0, x1 - if to_left { bend } else { -bend }, y1, x1, y1];
            out.push((std::array::from_fn(|i| w[i] * v.z + if i % 2 == 0 { v.x } else { v.y }), s.color));
        }
        out
    }

    fn pane_el(&mut self, r: &Rect, multi: bool, cx: &mut Context<Self>) -> AnyElement {
        let pane = r.id;
        let v = self.pv(pane).v;
        let cards = self.render_cards(pane, r.w, r.h, cx);
        let tools = self.selection_tools(pane, cx);
        let g = { let mut g = 24. * v.z; while g < 14. { g *= 4.; } g };
        let (vx, vy, w, h) = (v.x, v.y, r.w, r.h);
        let links = if v.z < FAR_Z { Vec::new() } else { self.links(pane, v) };
        let dash = [px(6.), px(5.)];
        // the dot grid, as a few tiles of dots (spacing rounded to whole device pixels)
        let sf = self.scale;
        let gap = (g * sf).round().max(2.) as u32;
        let dots = self.dot_tile(gap);
        let backdrop = canvas(|_, _, _| (), move |b: Bounds<Pixels>, _, window, _| {
            if let Some((img, per)) = dots {
                let gl = gap as f32 / sf;
                let side = gl * per as f32;
                let snap = |v: f32| (v * sf).round() / sf;
                // a dot sits at every (vx, vy) + k * gl; tiles hold their dots half a spacing in
                let (x0, y0) = (vx.rem_euclid(gl) - gl / 2. - side, vy.rem_euclid(gl) - gl / 2. - side);
                let mut y = y0;
                while y < h {
                    let mut x = x0;
                    while x < w {
                        let _ = window.paint_image(Bounds::new(point(px(snap(fl(b.origin.x) + x)), px(snap(fl(b.origin.y) + y))), size(px(side), px(side))), gpui::Corners::default(), img.clone(), 0, false);
                        x += side;
                    }
                    y += side;
                }
            }
            for (c, color) in &links {
                let at = |i: usize| point(b.origin.x + px(c[i]), b.origin.y + px(c[i + 1]));
                let mut p = PathBuilder::stroke(px(1.5)).dash_array(&dash);
                p.move_to(at(0));
                p.cubic_bezier_to(at(6), at(2), at(4));
                if let Ok(path) = p.build() { window.paint_path(path, cola(*color, 0.55)); }
            }
        }).absolute().size_full();
        let on_screen = |wx: f32, wy: f32| (wx * v.z + v.x, wy * v.z + v.y);
        // cards that just merged away, fading where they stood
        let fades: Vec<AnyElement> = self.fx.fades.iter().filter(|f| self.shared || f.pane == pane).map(|f| {
            let (x, y) = on_screen(f.x, f.y);
            self.card_box(x, y, f.w * v.z, f.h * v.z, v.z, mix(f.color, BORDER, 0.55)).opacity(1. - ease_out(ms_since(f.at) / 250.))
                .child(div().h(px(HEAD * v.z)).pl(px(11. * v.z)).flex().items_center().bg(mix(f.color, HEADC, 0.13)).text_size(tpx(12.5 * v.z)).child(f.title.clone())).into_any_element()
        }).collect();
        // lines flying between cards
        let mut flies: Vec<AnyElement> = Vec::new();
        for f in self.fx.flies.iter().filter(|f| self.shared || f.pane == pane) {
            for (k, (text, hl)) in f.rows.iter().enumerate() {
                let (x, y) = on_screen(f.pos(k).0, f.pos(k).1);
                flies.push(div().absolute().left(px(x)).top(px(y)).h(px(LH * v.z)).pr(px(8. * v.z)).rounded(px(3. * v.z)).bg(mix(f.color, CARD, 0.18)).font_family(mono()).text_size(tpx(12.5 * v.z)).line_height(px(LH * v.z))
                    .text_color(col(FG)).whitespace_nowrap().child(StyledText::new(text.clone()).with_highlights(hl.clone())).into_any_element());
            }
        }
        let is_active = multi && self.active == pane;
        let marquee = match self.drag {
            Some(Drag::Marquee { pane: p, sx, sy, cx: mx, cy: my, moved: true, .. }) if p == pane => Some(div().absolute().left(px(sx.min(mx) - r.x)).top(px(sy.min(my) - r.y)).w(px((sx - mx).abs())).h(px((sy - my).abs())).border_1().border_color(col(ACCENT)).bg(cola(ACCENT, 0.22))),
            _ => None,
        };
        // where a picked snippet would land: an insert line, or the range it replaces
        let count = self.pick.as_ref().map_or(0, |p| p.snippets.len());
        let pick_mark = self.pick.as_ref().and_then(|p| p.over).and_then(|(f, at, del)| self.doc.card(f).map(|c| (c.clone(), at, del))).filter(|(c, ..)| self.in_pane(c, pane)).map(|(c, at, del)| {
            let (x, y) = on_screen(c.x, c.y + HEAD + PAD + at as f32 * LH);
            if del > 0 {
                return div().absolute().left(px(x)).top(px(y)).w(px(c.w * v.z)).h(px(del as f32 * LH * v.z)).bg(cola(BAD, 0.2)).border_1().border_dashed().border_color(col(BAD));
            }
            div().absolute().left(px(x)).top(px(y - 1.5)).w(px(c.w * v.z)).h(px(3.)).rounded(px(2.)).bg(col(SNIP)).shadow(vec![BoxShadow { color: cola(SNIP, 0.8), offset: point(px(0.), px(0.)), blur_radius: px(12.), spread_radius: px(0.) }])
                .child(div().absolute().left(px(-5.)).top(px(-4.)).size(px(11.)).rounded_full().bg(col(SNIP)))
                .child(div().absolute().right(px(8.)).top(px(-20.)).px(px(7.)).py(px(1.)).rounded(px(4.)).bg(col(SNIP)).text_color(col(0xffffff)).text_size(tpx(11.)).whitespace_nowrap()
                    .child(format!("merge {count} snippet{} at line {}", if count == 1 { "" } else { "s" }, at + 1)))
        });
        let group: SharedString = format!("pane{pane}").into();
        let bar = div().absolute().left(px(22.)).top(px(8.)).flex().gap(px(1.)).p(px(2.)).bg(cola(PANEL, 0.92)).border_1().border_color(col(PANEL_BORDER)).rounded(px(7.)).opacity(0.).group_hover(group.clone(), |s| s.opacity(1.))
            .child(tool("splitR", 13., MUTED, cx, move |t, _, _| { t.active = pane; t.split(true); }))
            .child(tool("splitD", 13., MUTED, cx, move |t, _, _| { t.active = pane; t.split(false); }))
            .when(multi, |d| d.child(tool("close", 13., MUTED, cx, move |t, _, _| t.close_pane(pane))));
        let panning = matches!(self.drag, Some(Drag::Pan { pane: p, .. }) if p == pane);
        div().id(("pane", pane)).group(group).absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).overflow_hidden().bg(col(BG))
            .when(panning, |d| d.cursor_grabbing()).when(self.space && !panning, |d| d.cursor_grab()).when(self.pick.is_some(), |d| d.cursor_crosshair())
            .on_scroll_wheel(cx.listener(move |t, e: &ScrollWheelEvent, _, cx| t.on_wheel(pane, e, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |t, e: &MouseDownEvent, _, cx| t.pane_mouse_down(pane, e, cx)))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |t, e: &MouseDownEvent, _, cx| t.pane_mouse_down(pane, e, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |t, e: &MouseDownEvent, _, cx| { t.active = pane; t.canvas_menu(pane, fl(e.position.x), fl(e.position.y)); cx.notify(); }))
            .child(backdrop).children(fades).children(cards).children(flies).children(marquee).children(pick_mark).children(tools).child(bar)
            .when(is_active, |d| d.child(div().absolute().size_full().border_1().border_color(cola(ACCENT, 0.55))))
            .into_any_element()
    }

    /// The little handles on the canvas edges that slide the explorer and the chat open or shut.
    fn edge_toggle(&self, left: bool, open: bool, cx: &mut Context<Self>) -> Div {
        let (x, y, w, h) = self.canvas_area();
        let arrow = if left == open { "◀" } else { "▶" };
        div().absolute().top(px(y + h / 2. - 22.)).w(px(14.)).h(px(44.)).flex().items_center().justify_center().bg(col(PANEL)).border_1().border_color(col(PANEL_BORDER)).text_size(tpx(9.)).text_color(col(MUTED))
            .cursor_pointer().hover(|s| s.text_color(col(FG)))
            .when(left, |d| d.left(px(x)).border_l_0().rounded_r(px(6.))).when(!left, |d| d.left(px(x + w - 14.)).border_r_0().rounded_l(px(6.)))
            .child(arrow)
            .on_mouse_down(MouseButton::Left, cx.listener(move |t, _: &MouseDownEvent, _, cx| { cx.stop_propagation(); if left { t.explorer_open = !t.explorer_open; } else { t.chat_open = !t.chat_open; } cx.notify(); }))
    }
}

impl Render for CanvasView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vs = window.viewport_size();
        self.win_size = (fl(vs.width), fl(vs.height));
        self.fx.tick();
        self.frame += 1;
        self.scale = window.scale_factor();
        self.tend_tiles();
        for img in std::mem::take(&mut self.dead_tiles) { cx.drop_image(img, Some(window)); }
        // the side panels slide open and shut; the canvas follows their edges
        let (ex, ch) = (self.explorer_open, self.chat_open);
        self.side = (EXPLORER_W * self.fx.tween(Tw::Explorer, if ex { 1. } else { 0. }, 200.), CHAT_W * self.fx.tween(Tw::Chat, if ch { 1. } else { 0. }, 200.));
        self.step();
        for id in self.pending_bottom.drain().collect::<Vec<_>>() { self.scroll_handle(id).scroll_to_bottom(); }
        let (rects, sashes) = self.pane_rects();
        let multi = rects.len() > 1;
        let panes: Vec<AnyElement> = rects.iter().map(|r| self.pane_el(r, multi, cx)).collect();
        let sash_els: Vec<AnyElement> = sashes.into_iter().map(|s| {
            let (x, y, w, h, hz) = (s.x, s.y, s.w, s.h, s.horizontal);
            div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).bg(col(PANEL_BORDER)).hover(|d| d.bg(col(ACCENT))).when(hz, |d| d.cursor_col_resize()).when(!hz, |d| d.cursor_row_resize())
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
        let explorer = (self.side.0 > 0.5).then(|| div().absolute().left_0().top(px(TOP)).bottom_0().w(px(self.side.0)).overflow_hidden().child(self.explorer(cx)));
        let chat = (self.side.1 > 0.5).then(|| div().absolute().right_0().top(px(TOP)).bottom_0().w(px(self.side.1)).overflow_hidden().child(self.global_chat(cx)));
        let toggles = [self.edge_toggle(true, ex, cx), self.edge_toggle(false, ch, cx)];
        let ctx = self.context_panel(cx);
        let overlays = self.overlays_ui(cx);
        if std::mem::take(&mut self.fx.want_frame) { window.request_animation_frame(); }
        div().id("root").track_focus(&self.focus).on_key_down(cx.listener(Self::on_key)).on_key_up(cx.listener(Self::on_key_up))
            .on_mouse_move(cx.listener(Self::on_move)).on_mouse_up(MouseButton::Left, cx.listener(Self::on_up)).on_mouse_up(MouseButton::Middle, cx.listener(Self::on_up))
            .relative().size_full().bg(col(BG)).text_color(col(FG)).text_size(tpx(13.))
            .children(panes).children(sash_els).child(top).children(explorer).children(chat).children(toggles).child(ctx).children(overlays)
    }
}
