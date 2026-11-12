//! Card text drawn into bitmaps on background threads, and reused.
//!
//! GPUI paints text glyph by glyph on every frame, which made scrolling and zooming drag with several
//! files in view. Here a card's text is cut into a grid of equal cells ([`CELL_W`] × [`CELL_H`] device
//! pixels at a given zoom), each rasterised once on a worker thread and drawn as one textured quad.
//! The UI never waits for rasterising; cells off screen are never drawn.
//!
//! While a zoom glides, workers keep producing cells at zoom steps a little ahead of the glide, and a
//! card is drawn from the most complete set of cells at or just above the current zoom, scaled on the
//! GPU; it is only ever enlarged briefly, until the next step's cells arrive, so text stays sharp
//! instead of going soft until the zoom settles. Once the view rests, cells at
//! exactly its zoom take over and land on whole device pixels. Text is never simplified.

use crate::highlight::Tok;
use crate::model::Lines;
use cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, SwashCache, fontdb};
use gpui::RenderImage;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// Every tile is a cell of this many device pixels in a grid over a card's text, at one zoom.
/// One size for all of them matters for memory: GPUI keeps every staging buffer it has made for an
/// upload and reuses one only for an upload no larger than it, and packs images into 1024×1024
/// texture pages. Equal cells reuse those buffers exactly and fill the pages evenly.
pub(crate) const CELL_W: u32 = 256;
pub(crate) const CELL_H: u32 = 256;
pub(crate) const CELL_BYTES: usize = (CELL_W * CELL_H * 4) as usize;
/// New cells handed to the GPU per frame; the rest wait for the next frame.
pub(crate) const UPLOAD_PER_FRAME: usize = 32;
/// (view, card, zoom bits, column, row).
pub(crate) type Key = (u64, u64, u32, u32, u32);

/// One cell, or the promise of one while a worker draws it.
pub(crate) struct Cell {
    pub lines: Lines,
    pub start_no: usize,
    pub scale: f32,
    pub img: Option<Arc<RenderImage>>,
    /// Its text holds a character the code font lacks: those lines are drawn directly, where GPUI's
    /// font fallback applies.
    pub missing: bool,
    pub asked: bool,
    pub used: u64,
}

impl Cell {
    pub fn bytes(&self) -> usize { if self.img.is_some() { CELL_BYTES } else { 0 } }
}

/// Work for a rasteriser thread: one cell. Sizes are in world units; the worker scales them by
/// `z * scale` and draws the part of lines `range` that falls in the cell.
pub(crate) struct Job {
    pub key: Key,
    pub lines: Lines,
    pub range: Range<usize>,
    pub spans: crate::highlight::Spans,
    pub start_no: usize,
    pub z: f32,
    pub scale: f32,
    pub gutter: f32,
    pub num_right: f32,
    pub cw: f32,
    pub lh: f32,
}

pub(crate) struct Done {
    pub key: Key,
    pub lines: Lines,
    pub start_no: usize,
    pub scale: f32,
    /// `None`: the code font lacks one of the characters.
    pub img: Option<Arc<RenderImage>>,
}

/// The rasteriser threads.
pub(crate) struct Pool {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    pub in_flight: usize,
    pub threads: usize,
}

impl Pool {
    /// Starts the workers. The system fonts are searched once, off the UI thread, for the code font's
    /// files; each worker then loads only those two faces.
    pub fn new(family: &'static str) -> Self {
        let threads = std::thread::available_parallelism().map_or(2, |n| (n.get() / 3).clamp(1, 4));
        let (jobs, job_rx) = channel::<Job>();
        let (done_tx, done) = channel::<Done>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        std::thread::spawn(move || {
            let Some(sources) = Raster::find(family) else { return };
            for _ in 0..threads {
                let (rx, tx, sources) = (job_rx.clone(), done_tx.clone(), sources.clone());
                std::thread::spawn(move || {
                    let Some(mut r) = Raster::with(&sources) else { return };
                    loop {
                        let Ok(job) = rx.lock().map(|g| g.recv()) else { return };
                        let Ok(job) = job else { return };
                        if tx.send(r.run(job)).is_err() { return; }
                    }
                });
            }
        });
        Self { jobs, done, in_flight: 0, threads }
    }

    pub fn send(&mut self, job: Job) { if self.jobs.send(job).is_ok() { self.in_flight += 1; } }

    pub fn finished(&mut self) -> Vec<Done> {
        let out: Vec<Done> = self.done.try_iter().collect();
        self.in_flight = self.in_flight.saturating_sub(out.len());
        out
    }

    /// Jobs allowed in flight at once: enough to keep every worker busy, few enough that what they draw
    /// is still wanted when it arrives.
    pub fn room(&self) -> bool { self.in_flight < self.threads * 12 }
}

struct Glyph { left: i32, top: i32, w: usize, h: usize, alpha: Vec<u8> }

/// A glyph rasteriser (the same library GPUI uses on Linux), one per worker.
struct Raster {
    fonts: FontSystem,
    swash: SwashCache,
    regular: fontdb::ID,
    italic: fontdb::ID,
    glyphs: HashMap<CacheKey, Option<Glyph>>,
}

fn rgb(c: u32) -> [u8; 3] { [(c >> 16) as u8, (c >> 8) as u8, c as u8] }

impl Raster {
    /// The files of the code font's upright and italic faces.
    fn find(family: &str) -> Option<[(fontdb::Source, u32); 2]> {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let q = |style| db.query(&fontdb::Query { families: &[fontdb::Family::Name(family)], weight: fontdb::Weight::NORMAL, stretch: fontdb::Stretch::Normal, style });
        let regular = q(fontdb::Style::Normal)?;
        let italic = q(fontdb::Style::Italic).or_else(|| q(fontdb::Style::Oblique)).unwrap_or(regular);
        let face = |id| db.face(id).map(|f| (f.source.clone(), f.index));
        Some([face(regular)?, face(italic)?])
    }

    /// A font system holding just those faces (not the thousands of system fonts).
    fn with(sources: &[(fontdb::Source, u32); 2]) -> Option<Self> {
        let mut db = fontdb::Database::new();
        let ids: Vec<fontdb::ID> = sources.iter().map(|(src, index)| {
            let loaded = db.load_font_source(src.clone());
            loaded.iter().copied().find(|id| db.face(*id).is_some_and(|f| f.index == *index)).or(loaded.first().copied())
        }).collect::<Option<_>>()?;
        let fonts = FontSystem::new_with_locale_and_db("en-US".into(), db);
        Some(Self { fonts, swash: SwashCache::new(), regular: ids[0], italic: ids[1], glyphs: HashMap::new() })
    }

    fn run(&mut self, job: Job) -> Done {
        let img = self.cell(&job).and_then(|px| image(CELL_W, CELL_H, px));
        Done { key: job.key, lines: job.lines, start_no: job.start_no, scale: job.scale, img }
    }

    /// Rasterises one cell as BGRA with straight alpha. `None` if a character in it isn't in the code font.
    fn cell(&mut self, job: &Job) -> Option<Vec<u8>> {
        if self.glyphs.len() > 6000 { self.glyphs.clear(); }
        let (regular, italic) = (self.fonts.get_font(self.regular)?, self.fonts.get_font(self.italic)?);
        let k = job.z * job.scale;
        let (lh, cw, gutter, num_right, font_px) = (job.lh * k, job.cw * k, job.gutter * k, job.num_right * k, 12.5 * k);
        let (x0, y0) = ((job.key.3 * CELL_W) as f32, (job.key.4 * CELL_H) as f32);
        let (w, h) = (CELL_W as usize, CELL_H as usize);
        let m = regular.as_swash().metrics(&[]).scale(font_px);
        let mut px = vec![0u8; w * h * 4];
        let (fg, num, num_alpha) = (rgb(0xd6d9de), rgb(0x8a9099), 0.7f32);
        for i in job.range.clone() {
            let line = &job.lines[i];
            // whole device pixels for every baseline, the same in every cell, so cells line up
            let baseline = ((i as f32 * lh + (lh - m.ascent - m.descent) / 2. + m.ascent).round() - y0) as i32;
            if x0 < gutter {
                let no = (job.start_no + i).to_string();
                let mut x = num_right - no.len() as f32 * cw - x0;
                for ch in no.chars() {
                    let gid = regular.as_swash().charmap().map(ch);
                    self.blit(&mut px, w, h, self.regular, gid, font_px, x, baseline, num, num_alpha);
                    x += cw;
                }
            }
            let spans = job.spans.line(i - job.range.start).unwrap_or(&[]);
            let (mut col, mut sp) = (0usize, 0usize);
            for (byte, ch) in line.char_indices() {
                if ch == '\t' { col += 4; continue; }
                if ch == ' ' { col += 1; continue; }
                let x = gutter + col as f32 * cw - x0;
                col += 1;
                if x < -2. * cw { continue; }
                if x >= w as f32 { break; }
                while sp < spans.len() && byte >= spans[sp].end as usize { sp += 1; }
                let tok = spans.get(sp).filter(|s| byte >= s.start as usize).map(|s| s.tok);
                let slanted = tok == Some(Tok::Comment);
                let (font_id, gid) = match (if slanted { &italic } else { &regular }).as_swash().charmap().map(ch) {
                    0 => match regular.as_swash().charmap().map(ch) { 0 => return None, g => (self.regular, g) },
                    g => (if slanted { self.italic } else { self.regular }, g),
                };
                self.blit(&mut px, w, h, font_id, gid, font_px, x, baseline, tok.map_or(fg, |t| rgb(tok_rgb(t))), 1.);
            }
        }
        Some(px)
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

/// A square of the canvas's dot grid, `per` dots a side at `gap` device pixels apart, each a 2×2
/// block centred in its spot. Drawing the grid as a few of these instead of a quad per dot takes
/// thousands of primitives out of every frame.
pub(crate) fn dot_tile(gap: u32, per: u32, rgb_: u32, alpha: f32) -> Option<Arc<RenderImage>> {
    let side = (gap * per) as usize;
    let mut px = vec![0u8; side * side * 4];
    let [r, g, b] = rgb(rgb_);
    let a = (alpha * 255.).round() as u8;
    for j in 0..per as usize {
        for i in 0..per as usize {
            let (cx, cy) = (i * gap as usize + gap as usize / 2, j * gap as usize + gap as usize / 2);
            for y in cy.saturating_sub(1)..(cy + 1).min(side) {
                for x in cx.saturating_sub(1)..(cx + 1).min(side) {
                    px[(y * side + x) * 4..][..4].copy_from_slice(&[b, g, r, a]);
                }
            }
        }
    }
    image(side as u32, side as u32, px)
}

/// Wraps tile pixels as an image GPUI can draw.
pub(crate) fn image(w: u32, h: u32, bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
    let buf = image::RgbaImage::from_raw(w, h, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}
