//! Motion and icons. Effects are presentation only: the document is already committed when one starts.

use super::{GP, LH};
use gpui::{AssetSource, Hsla, HighlightStyle, SharedString, Svg, prelude::*, px, svg};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

pub(crate) fn ms_since(t: Instant) -> f32 { Instant::now().saturating_duration_since(t).as_secs_f32() * 1000. }
pub(crate) fn later(ms: f32) -> Instant { Instant::now() + Duration::from_secs_f32(ms.max(0.) / 1000.) }

/// CSS `cubic-bezier(x1, y1, x2, y2)` evaluated at `x`.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    if x <= 0. { return 0.; }
    if x >= 1. { return 1.; }
    let f = |a: f32, b: f32, t: f32| { let u = 1. - t; 3. * u * u * t * a + 3. * u * t * t * b + t * t * t };
    let (mut lo, mut hi, mut t) = (0f32, 1f32, x);
    for _ in 0..14 { if f(x1, x2, t) < x { lo = t; } else { hi = t; } t = (lo + hi) / 2.; }
    f(y1, y2, t)
}
/// The glide every layout change uses.
pub(crate) fn glide(t: f32) -> f32 { bezier(0.2, 0.8, 0.3, 1.0, t) }
fn fly_curve(t: f32) -> f32 { bezier(0.3, 0.7, 0.2, 1.0, t) }
pub(crate) fn ease_out(t: f32) -> f32 { let u = 1. - t.clamp(0., 1.); 1. - u * u }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Tw { Drawer(u64), Draft(u64), Explorer, Chat, Dir(u64), Ghost }

struct Tween { from: f32, to: f32, at: Instant, ms: f32 }
impl Tween {
    fn value(&self) -> f32 { self.from + (self.to - self.from) * glide(ms_since(self.at) / self.ms) }
    fn done(&self) -> bool { ms_since(self.at) >= self.ms }
}

pub(crate) type Rich = (SharedString, Vec<(Range<usize>, HighlightStyle)>);

/// Lines flying from one card to another (world coordinates of the first line's code column).
pub(crate) struct Fly { pub pane: u64, pub rows: Vec<Rich>, pub from: (f32, f32), pub to: (f32, f32), pub color: u32, pub at: Instant }
impl Fly {
    fn stagger(&self) -> f32 { (260. / self.rows.len().max(1) as f32).min(16.) }
    pub fn total(&self) -> f32 { 440. + self.rows.len() as f32 * self.stagger() }
    /// World position of row `k` right now.
    pub fn pos(&self, k: usize) -> (f32, f32) {
        let p = fly_curve((ms_since(self.at) - k as f32 * self.stagger()) / 420.);
        (self.from.0 + (self.to.0 - self.from.0) * p, self.from.1 + (self.to.1 - self.from.1) * p + k as f32 * LH)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum LineKind {
    /// `n` lines were inserted at `at`; the gap opens, then they show after `hide` ms.
    Grow,
    /// `n` lines were removed at `at`; the gap closes.
    Shut,
    /// A dragged block was dropped at `at`; its breathing room closes.
    Settle,
    /// Changed lines glow green for a moment.
    Flash,
}
pub(crate) struct LineFx { pub file: u64, pub at: usize, pub n: usize, pub kind: LineKind, pub start: Instant, pub hide: f32, /// Sideways distance a dropped block still has to glide.
    pub dx: f32 }
impl LineFx {
    fn ms(&self) -> f32 { match self.kind { LineKind::Grow | LineKind::Shut => 260., LineKind::Settle => 200., LineKind::Flash => 1400. } }
    fn left(&self) -> f32 { 1. - glide(ms_since(self.start) / self.ms()) }
    fn done(&self) -> bool { ms_since(self.start) >= self.ms().max(self.hide + 140.) }
}

/// A card that just went away (merged into a file), fading where it stood.
pub(crate) struct Fade { pub pane: u64, pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub color: u32, pub title: String, pub at: Instant }

pub(crate) struct Fx {
    tweens: HashMap<Tw, Tween>,
    pub flies: Vec<Fly>,
    pub lines: Vec<LineFx>,
    pub fades: Vec<Fade>,
    /// Cards fading in; a start in the future keeps the card hidden until then.
    pub born: HashMap<u64, Instant>,
    /// Cards gliding in from an offset (Arrange).
    pub slides: HashMap<u64, (f32, f32, Instant)>,
    /// Regions whose tabs and marker fade in after a drop.
    pub fresh: HashMap<u64, Instant>,
    pub want_frame: bool,
    /// Frame time relative to 60 Hz, so per-frame easing runs at the same speed on any display.
    pub k: f32,
    last: Instant,
}

impl Default for Fx {
    fn default() -> Self {
        Self { tweens: HashMap::new(), flies: Vec::new(), lines: Vec::new(), fades: Vec::new(), born: HashMap::new(), slides: HashMap::new(), fresh: HashMap::new(), want_frame: false, k: 1., last: Instant::now() }
    }
}

impl Fx {
    /// Once per frame: measure the frame and drop finished effects.
    pub fn tick(&mut self) {
        let now = Instant::now();
        self.k = (now.duration_since(self.last).as_secs_f32() * 60.).clamp(0.25, 3.);
        self.last = now;
        self.flies.retain(|f| ms_since(f.at) < f.total());
        self.lines.retain(|l| !l.done());
        self.fades.retain(|f| ms_since(f.at) < 250.);
        self.born.retain(|_, t| ms_since(*t) < 260.);
        self.slides.retain(|_, s| ms_since(s.2) < 380.);
        self.fresh.retain(|_, t| ms_since(*t) < 420.);
        if !(self.flies.is_empty() && self.lines.is_empty() && self.fades.is_empty() && self.born.is_empty() && self.slides.is_empty() && self.fresh.is_empty()) { self.want_frame = true; }
    }

    /// The fraction to move toward a target this frame, given the fraction a 60 Hz frame would move.
    pub fn step(&self, per_frame: f32) -> f32 { 1. - (1. - per_frame).powf(self.k) }

    /// A value that glides to `target` whenever it changes. The first sight of a key is not animated.
    pub fn tween(&mut self, key: Tw, target: f32, ms: f32) -> f32 {
        let t = self.tweens.entry(key).or_insert(Tween { from: target, to: target, at: Instant::now(), ms });
        if t.to != target { *t = Tween { from: t.value(), to: target, at: Instant::now(), ms }; }
        if t.done() { return t.to; }
        self.want_frame = true;
        t.value()
    }
    pub fn forget(&mut self, key: Tw) { self.tweens.remove(&key); }

    /// Pixels that line `i` of `file` is displaced by running effects.
    pub fn shift(&self, file: u64, i: usize) -> f32 {
        let mut px_ = 0.;
        for l in self.lines.iter().filter(|l| l.file == file) {
            let left = l.left();
            match l.kind {
                LineKind::Grow => if i >= l.at + l.n { px_ -= left * l.n as f32 * LH; },
                LineKind::Shut => if i >= l.at { px_ += left * l.n as f32 * LH; },
                LineKind::Settle => if i >= l.at + l.n { px_ += left * 2. * GP; } else if i >= l.at { px_ += left * GP; },
                LineKind::Flash => {}
            }
        }
        px_
    }
    /// Extra height the card has while effects run.
    pub fn extra(&self, file: u64) -> f32 {
        self.lines.iter().filter(|l| l.file == file).map(|l| match l.kind {
            LineKind::Grow => -l.left() * l.n as f32 * LH,
            LineKind::Shut => l.left() * l.n as f32 * LH,
            LineKind::Settle => l.left() * 2. * GP,
            LineKind::Flash => 0.,
        }).sum()
    }
    /// Opacity of a line (new lines stay hidden until the flying copies land).
    pub fn alpha(&self, file: u64, i: usize) -> f32 {
        self.lines.iter().filter(|l| l.file == file && l.kind == LineKind::Grow && i >= l.at && i < l.at + l.n)
            .map(|l| ((ms_since(l.start) - l.hide) / 120.).clamp(0., 1.)).fold(1., f32::min)
    }
    /// Sideways offset of a line in a block that was just dropped.
    pub fn dx(&self, file: u64, i: usize) -> f32 {
        self.lines.iter().filter(|l| l.file == file && l.kind == LineKind::Settle && i >= l.at && i < l.at + l.n).map(|l| l.dx * l.left()).sum()
    }
    pub fn flash(&self, file: u64, i: usize) -> f32 {
        self.lines.iter().filter(|l| l.file == file && l.kind == LineKind::Flash && i >= l.at && i < l.at + l.n).map(|l| 0.35 * (1. - ease_out(ms_since(l.start) / 1400.))).fold(0., f32::max)
    }
    pub fn line(&mut self, file: u64, at: usize, n: usize, kind: LineKind, hide: f32) {
        if n > 0 { self.lines.push(LineFx { file, at, n, kind, start: Instant::now(), hide, dx: 0. }); self.want_frame = true; }
    }

    /// (opacity, downward offset in world px) of a card that is appearing.
    pub fn card(&self, id: u64) -> (f32, f32) {
        match self.born.get(&id) {
            Some(t) if Instant::now() < *t => (0., 0.),
            Some(t) => { let p = ease_out(ms_since(*t) / 240.); (0.3 + 0.7 * p, 8. * (1. - p)) }
            None => (1., 0.),
        }
    }
    pub fn slide(&self, id: u64) -> (f32, f32) {
        self.slides.get(&id).map_or((0., 0.), |(dx, dy, t)| { let left = 1. - glide(ms_since(*t) / 380.); (dx * left, dy * left) })
    }
    pub fn fresh_alpha(&self, id: u64) -> f32 { self.fresh.get(&id).map_or(1., |t| if Instant::now() < *t { 0. } else { ((ms_since(*t) - 50.) / 350.).clamp(0., 1.) }) }
}

// ---------- icons ----------

/// The same line icons the VS Code extension draws, served to GPUI as SVG assets.
pub struct Icons;

fn icon_body(name: &str) -> Option<&'static str> {
    Some(match name {
        "close" => r#"<path d="M6 6l12 12M18 6L6 18"/>"#,
        "ctx" => r#"<circle cx="12" cy="12" r="3"/><circle cx="12" cy="12" r="8"/>"#,
        "explain" => r#"<path d="M9.1 9a3 3 0 1 1 3.9 2.9c-.6.2-1 .8-1 1.4V14"/><circle cx="12" cy="17.5" r=".6"/><circle cx="12" cy="12" r="10"/>"#,
        "editor" => r#"<path d="M14 3h7v7M10 14 21 3M21 14v5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5"/>"#,
        "save" => r#"<path d="M5 3h11l5 5v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"/><path d="M7 3v5h8M7 21v-7h10v7"/>"#,
        "chat" => r#"<path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1-4.6A8 8 0 1 1 21 12z"/>"#,
        "merge" => r#"<path d="M6 3v6a6 6 0 0 0 6 6h6M15 12l3 3-3 3"/>"#,
        "send" => r#"<path d="M5 12h14M13 6l6 6-6 6"/>"#,
        "stop" => r#"<rect x="7" y="7" width="10" height="10" rx="1.5"/>"#,
        "trash" => r#"<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>"#,
        "notes" => r#"<path d="M4 6h16M4 12h10M4 18h7"/>"#,
        "fit" => r#"<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>"#,
        "gear" => r#"<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>"#,
        "sparkle" => r#"<path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"/><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"/>"#,
        "reset" => r#"<path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5"/>"#,
        "refresh" => r#"<path d="M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5"/>"#,
        "splitR" => r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M12 4v16"/>"#,
        "splitD" => r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 12h18"/>"#,
        "logo" => r#"<rect x="2.5" y="3.5" width="9" height="7" rx="1.5"/><rect x="13.5" y="9.5" width="8" height="6" rx="1.5"/><rect x="5.5" y="15" width="8" height="5.5" rx="1.5"/><path d="M11.5 7h2.5v2.5M9.5 15v-4.5"/>"#,
        "chev" => r#"<path d="M9 6l6 6-6 6"/>"#,
        "check" => r#"<path d="M5 12.5l4.5 4.5L19 7.5"/>"#,
        "draft" => r#"<path d="M4 20h4L19 9l-4-4L4 16v4zM13.5 6.5l4 4"/>"#,
        "apply" => r#"<path d="M12 4v11M7 11l5 5 5-5M5 20h14"/>"#,
        "ref" => r#"<path d="M9 5h10v10M19 5L6 18"/>"#,
        "tri" => r#"<path d="M8 5l11 7-11 7z" fill="black"/>"#,
        _ => return None,
    })
}

impl AssetSource for Icons {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let Some(body) = path.strip_prefix("icons/").and_then(icon_body) else { return Ok(None) };
        let width = if path.ends_with("logo") { "1.8" } else { "2" };
        Ok(Some(Cow::Owned(format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#).into_bytes())))
    }
    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> { Ok(Vec::new()) }
}

pub(crate) fn icon(name: &str, size: f32, color: Hsla) -> Svg { svg().path(format!("icons/{name}")).size(px(size)).flex_none().text_color(color) }

/// The colour dot the VS Code extension shows per file type.
pub(crate) fn ext_color(path: &str) -> u32 {
    let base = path.rsplit('/').next().unwrap_or(path);
    let ext = base.rsplit_once('.').filter(|(a, _)| !a.is_empty()).map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "ts" | "tsx" => 0x3178c6, "js" | "jsx" | "mjs" | "cjs" => 0xf1e05a, "json" => 0xa1a1a1, "py" => 0x3572a5, "rs" => 0xdea584, "go" => 0x00add8,
        "c" | "h" => 0x8e9aaf, "cpp" | "hpp" | "cc" => 0xf34b7d, "cs" => 0x178600, "java" => 0xb07219, "kt" => 0xa97bff, "swift" => 0xf05138, "rb" => 0xcc342d,
        "php" => 0x4f5d95, "lua" => 0x000080, "html" => 0xe34c26, "css" => 0x563d7c, "scss" => 0xc6538c, "md" => 0x4a90d9, "yml" | "yaml" => 0xcb171e, "toml" => 0x9c4221,
        "sh" => 0x89e051, "ps1" => 0x012456, "sql" => 0xe38c00, "vue" => 0x41b883, "svelte" => 0xff3e00, "scm" => 0x1e4aec,
        _ => 0x7f8794,
    }
}
