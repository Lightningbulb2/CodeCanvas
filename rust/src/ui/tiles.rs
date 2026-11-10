//! Card text drawn once into bitmaps ("tiles") and reused.
//!
//! GPUI paints text glyph by glyph on every frame, which is what made scrolling and zooming drag with
//! several files in view. Here each run of [`TILE`] lines is rasterised into one image, at the zoom the
//! view is resting at, and then drawn as a single textured quad. While a zoom glides the existing tiles
//! are simply scaled, the way a browser scales a layer, and they are redrawn sharp once it settles.
//! Text is never simplified: a tile holds the same glyphs the direct path would draw.

use crate::highlight::Tok;
use crate::model::Lines;
use cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, SwashCache, fontdb};
use gpui::RenderImage;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

/// Lines per tile.
pub(crate) const TILE: usize = 48;
/// No tile is wider than this many device pixels; longer lines are cut off (cards are narrower anyway).
const MAX_W: f32 = 4096.;

/// One cached tile. `img` is `None` when its lines hold characters the code font lacks; such lines are
/// always drawn directly, where GPUI's font fallback applies.
pub(crate) struct Tile {
    pub lines: Lines,
    pub z: f32,
    pub scale: f32,
    pub start_no: usize,
    pub img: Option<Arc<RenderImage>>,
    /// Size in logical pixels at zoom `z`.
    pub w: f32,
    pub h: f32,
    pub bytes: usize,
    pub used: u64,
}

struct Glyph { left: i32, top: i32, w: usize, h: usize, alpha: Vec<u8> }

/// The app's own glyph rasteriser (the same library GPUI uses on Linux), for filling tiles.
pub(crate) struct Raster {
    fonts: FontSystem,
    swash: SwashCache,
    regular: fontdb::ID,
    italic: fontdb::ID,
    glyphs: HashMap<CacheKey, Option<Glyph>>,
}

/// Everything needed to draw one tile, in device pixels.
pub(crate) struct Spec<'a> {
    pub lines: &'a [String],
    pub spans: &'a [Vec<(Range<usize>, Tok)>],
    pub first_no: usize,
    pub gutter: f32,
    /// Right edge of the line numbers.
    pub num_right: f32,
    pub cw: f32,
    pub lh: f32,
    pub font_px: f32,
}

fn rgb(c: u32) -> [u8; 3] { [(c >> 16) as u8, (c >> 8) as u8, c as u8] }

impl Raster {
    /// Loads the system fonts; slow enough to be done off the UI thread.
    pub fn new(family: &str) -> Option<Self> {
        let fonts = FontSystem::new();
        let find = |style| fonts.db().query(&fontdb::Query { families: &[fontdb::Family::Name(family)], weight: fontdb::Weight::NORMAL, stretch: fontdb::Stretch::Normal, style });
        let regular = find(fontdb::Style::Normal)?;
        let italic = find(fontdb::Style::Italic).or_else(|| find(fontdb::Style::Oblique)).unwrap_or(regular);
        Some(Self { fonts, swash: SwashCache::new(), regular, italic, glyphs: HashMap::new() })
    }

    /// Rasterises a tile as BGRA with straight alpha. `None` if a character isn't in the code font.
    pub fn tile(&mut self, s: &Spec) -> Option<(u32, u32, Vec<u8>)> {
        if self.glyphs.len() > 40_000 { self.glyphs.clear(); }
        let (regular, italic) = (self.fonts.get_font(self.regular)?, self.fonts.get_font(self.italic)?);
        let m = regular.as_swash().metrics(&[]).scale(s.font_px);
        let cols = s.lines.iter().map(|l| crate::ui::visual_len(l)).max().unwrap_or(0);
        let w = (s.gutter + cols as f32 * s.cw + 2.).min(MAX_W).ceil().max(1.) as usize;
        let h = (s.lines.len() as f32 * s.lh).ceil().max(1.) as usize;
        let mut px = vec![0u8; w * h * 4];
        let (fg, num, num_alpha) = (rgb(0xd6d9de), rgb(0x8a9099), 0.7f32);
        for (row, line) in s.lines.iter().enumerate() {
            let baseline = (row as f32 * s.lh + (s.lh - m.ascent - m.descent) / 2. + m.ascent).round() as i32;
            // the line number, right-aligned in the gutter
            let no = (s.first_no + row).to_string();
            let mut x = s.num_right - no.len() as f32 * s.cw;
            for ch in no.chars() {
                let gid = regular.as_swash().charmap().map(ch);
                self.blit(&mut px, w, h, self.regular, gid, s.font_px, x, baseline, num, num_alpha);
                x += s.cw;
            }
            let spans = s.spans.get(row).map_or(&[][..], |v| v.as_slice());
            let (mut col, mut sp) = (0usize, 0usize);
            for (byte, ch) in line.char_indices() {
                if ch == '\t' { col += 4; continue; }
                if ch == ' ' { col += 1; continue; }
                while sp < spans.len() && byte >= spans[sp].0.end { sp += 1; }
                let tok = spans.get(sp).filter(|(r, _)| byte >= r.start).map(|(_, t)| *t);
                let slanted = tok == Some(Tok::Comment);
                let font = if slanted { &italic } else { &regular };
                let gid = match font.as_swash().charmap().map(ch) { 0 => match regular.as_swash().charmap().map(ch) { 0 => return None, _ => 0 }, g => g };
                let (font_id, gid) = if gid == 0 { (self.regular, regular.as_swash().charmap().map(ch)) } else { (if slanted { self.italic } else { self.regular }, gid) };
                let x = s.gutter + col as f32 * s.cw;
                if x < w as f32 { self.blit(&mut px, w, h, font_id, gid, s.font_px, x, baseline, tok.map_or(fg, |t| rgb(tok_rgb(t))), 1.); }
                col += 1;
            }
        }
        Some((w as u32, h as u32, px))
    }

    #[allow(clippy::too_many_arguments)]
    fn blit(&mut self, px: &mut [u8], w: usize, h: usize, font: fontdb::ID, gid: u16, size: f32, x: f32, baseline: i32, color: [u8; 3], strength: f32) {
        let (key, xi, _) = CacheKey::new(font, gid, size, (x, 0.), CacheKeyFlags::empty());
        if !self.glyphs.contains_key(&key) {
            let g = self.swash.get_image_uncached(&mut self.fonts, key).filter(|i| matches!(i.content, cosmic_text::SwashContent::Mask) && i.placement.width > 0 && i.placement.height > 0)
                .map(|i| Glyph { left: i.placement.left, top: i.placement.top, w: i.placement.width as usize, h: i.placement.height as usize, alpha: i.data });
            self.glyphs.insert(key, g);
        }
        let Some(Some(g)) = self.glyphs.get(&key) else { return };
        let (x0, y0) = (xi + g.left, baseline - g.top);
        // One pixel around the glyph also takes its colour (at zero alpha), so a scaled tile doesn't
        // blend the text toward black at its edges.
        for gy in -1..=g.h as i32 {
            let y = y0 + gy;
            if y < 0 || y >= h as i32 { continue; }
            for gx in -1..=g.w as i32 {
                let xx = x0 + gx;
                if xx < 0 || xx >= w as i32 { continue; }
                let inside = gy >= 0 && gx >= 0 && (gy as usize) < g.h && (gx as usize) < g.w;
                let a = if inside { (g.alpha[gy as usize * g.w + gx as usize] as f32 * strength) as u8 } else { 0 };
                let p = &mut px[(y as usize * w + xx as usize) * 4..][..4];
                if a > p[3] || p[3] == 0 { p[0] = color[2]; p[1] = color[1]; p[2] = color[0]; }
                if a > p[3] { p[3] = a; }
            }
        }
    }
}

pub(crate) fn tok_rgb(t: Tok) -> u32 {
    match t { Tok::Keyword => 0xc678dd, Tok::Str => 0x98c379, Tok::Num => 0xd19a66, Tok::Comment => 0x6b7280, Tok::Type => 0xe5c07b, Tok::Func => 0x61afef }
}

/// Wraps tile pixels as an image GPUI can draw.
pub(crate) fn image(w: u32, h: u32, bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
    let buf = image::RgbaImage::from_raw(w, h, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}
