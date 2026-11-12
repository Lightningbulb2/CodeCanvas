//! The CodeCanvas view: state, views/panes, pointer and keyboard handling, LLM plumbing.

mod find;
pub mod fx;
mod panels;
mod render;
mod tiles;

use crate::agent::{self, ChatRequest, Draft, Segment};
use crate::highlight::{Tok, highlight};
use crate::llm::{Cancel, Message, list_models};
use crate::model::*;
use crate::settings::Settings;
use crate::text::TextBuffer;
use crate::web::WebTools;
use fx::{Fade, Fly, Fx, LineKind, Tw, later};
use gpui::{
    ClipboardItem, Context, FocusHandle, KeyDownEvent, KeyUpEvent, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollHandle, ScrollWheelEvent, Window, font, px,
};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const LH: f32 = 19.0;
pub(crate) const HEAD: f32 = 35.0;
pub(crate) const PAD: f32 = 6.0;
pub(crate) const GP: f32 = 10.0;
pub(crate) const TOP: f32 = 38.0;
pub(crate) const EXPLORER_W: f32 = 250.0;
pub(crate) const CHAT_W: f32 = 360.0;
pub(crate) const TAB_GAP: f32 = 24.0;
pub(crate) const FAR_Z: f32 = 0.32;
const MIN_Z: f32 = 0.04;
const MAX_Z: f32 = 2.5;

pub(crate) type Act = Rc<dyn Fn(&mut CanvasView, &mut Window, &mut Context<CanvasView>)>;
pub(crate) use crate::highlight::Spans;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Input { Chat(u64), Ask(u64), DraftEdit(u64), Editor(u64), Filter, Find(u64), Replace(u64), Rename(u64), Setting(usize) }

#[derive(Clone, Copy)]
pub(crate) struct PaneView { pub v: View, pub t: View, pub anchor: (f32, f32) }

#[derive(Clone, Debug)]
pub(crate) struct Rect { pub id: u64, pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

#[derive(Clone, Debug)]
pub(crate) struct Sash { pub path: Vec<usize>, pub idx: usize, pub horizontal: bool, pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub span: f32 }

pub(crate) struct MoveItem { id: u64, dx: f32, dy: f32, ox: f32, oy: f32, pane: u64 }

pub(crate) enum Drag {
    Pan { pane: u64, sx: f32, sy: f32, ox: f32, oy: f32 },
    Marquee { pane: u64, sx: f32, sy: f32, cx: f32, cy: f32, base: HashSet<u64>, moved: bool },
    Move { items: Vec<MoveItem>, sx: f32, sy: f32, moved: bool, single: Option<u64> },
    Select { id: u64 },
    EditSelect { id: u64 },
    Resize { id: u64, sx: f32, ow: f32, z: f32 },
    Region { id: u64, via: Via, sx: f32, sy: f32, wx: f32, wy: f32, pane: u64 },
    Edge { id: u64, top: bool, orig: (usize, usize), moved: bool, sx: f32, sy: f32 },
    ShiftLines { id: u64, a: usize, b: usize, sy: f32, wy: f32, pane: u64 },
    Sash { sash: Sash, start: f32, a0: f32, pair: f32, sum: f32 },
    Explorer { path: String, sx: f32, sy: f32, moved: bool },
}

/// What a card's highlighting was computed from.
pub(crate) enum SpanKey { Shared(Lines), Print(u64) }

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Via { Tab, Bar, Lines, Shift }

/// A block being dragged over files: lines part around it in real time.
pub(crate) struct Live {
    pub n: usize,
    pub src: Option<(u64, usize, usize)>,
    pub region: Option<u64>,
    pub over: Option<u64>,
    pub at: usize,
    pub ref_mode: bool,
    pub lock: bool,
    pub ghost_card: Option<u64>,
    pub lines: Vec<String>,
    pub lang: String,
    pub title: String,
    pub color: u32,
    pub gx: f32,
    pub gy: f32,
    pub gw: f32,
    pub pane: u64,
}

/// The global chat panel as its own view, so GPUI can reuse last frame's drawing of it whenever
/// something else on screen changes (scrolling or zooming the canvas). It draws through the canvas
/// view, whose state it shows; the canvas tells it to redraw when that state changes (see `chat_key`).
pub(crate) struct ChatPanelView { canvas: gpui::WeakEntity<CanvasView> }

impl gpui::Render for ChatPanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let Some(canvas) = self.canvas.upgrade() else { return gpui::IntoElement::into_any_element(gpui::div()) };
        let (el, animate) = canvas.update(cx, |t, cx| {
            let before = std::mem::take(&mut t.fx.want_frame);
            let el = t.global_chat(cx);
            (el, std::mem::replace(&mut t.fx.want_frame, before))
        });
        // the typing dots animate: this view asks for its own next frame
        if animate { window.request_animation_frame(); }
        gpui::IntoElement::into_any_element(el)
    }
}

/// A piece of the window's chrome drawn as its own cached view, like the chat panel: redrawn only
/// when what it shows changes (see `chrome_keys`), not on every frame of canvas scrolling or zooming.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Part { Top, Explorer, Context }

pub(crate) struct ChromeView { canvas: gpui::WeakEntity<CanvasView>, part: Part }

impl gpui::Render for ChromeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let Some(canvas) = self.canvas.upgrade() else { return gpui::IntoElement::into_any_element(gpui::div()) };
        let part = self.part;
        let (el, animate) = canvas.update(cx, |t, cx| {
            let before = std::mem::take(&mut t.fx.want_frame);
            let el = match part { Part::Top => t.topbar(cx), Part::Explorer => t.explorer(cx), Part::Context => t.context_panel(cx) };
            (el, std::mem::replace(&mut t.fx.want_frame, before))
        });
        // e.g. a folder arrow turning: this view asks for its own next frame
        if animate { window.request_animation_frame(); }
        gpui::IntoElement::into_any_element(el)
    }
}

/// A chat's virtualized message list, and what it was built from.
pub(crate) struct ChatList { pub state: gpui::ListState, pub n: usize, pub last: Option<usize>, pub z: f32 }

pub(crate) struct Busy { pub text: String, pub status: String, pub cancel: Cancel }

pub(crate) enum Ev {
    Delta(u64, String),
    Status(u64, String),
    Done(u64, String, Vec<Draft>),
    Error(u64, String),
    Progress(String, Vec<Segment>),
    ExplainDone(String, Result<(), String>),
    Models(Vec<String>),
    SaveFailed(String),
}

pub(crate) struct MenuItem { pub label: String, pub act: Option<Act> }
pub(crate) struct Menu { pub x: f32, pub y: f32, pub items: Vec<MenuItem>, pub at: Instant }
/// `over` is (file, line, lines it would replace).
pub(crate) struct Pick { pub snippets: Vec<u64>, pub file: Option<u64>, pub over: Option<(u64, usize, usize)> }

pub struct CanvasView {
    pub(crate) project: Arc<Project>,
    pub(crate) settings: Settings,
    pub(crate) tree: Vec<String>,
    pub(crate) doc: Doc,
    pub(crate) layout: Layout,
    pub(crate) views: HashMap<u64, PaneView>,
    pub(crate) shared: bool,
    pub(crate) active: u64,
    pub(crate) drag: Option<Drag>,
    pub(crate) live: Option<Live>,
    pub(crate) selected: HashSet<u64>,
    pub(crate) picked: Option<u64>,
    pub(crate) hover_tab: Option<u64>,
    pub(crate) doomed: Vec<u64>,
    undo: Vec<Doc>,
    redo: Vec<Doc>,
    pub(crate) inputs: HashMap<Input, TextBuffer>,
    pub(crate) focus_input: Option<Input>,
    pub(crate) editing: Option<u64>,
    pub(crate) busy: HashMap<u64, Busy>,
    tx: Sender<Ev>,
    rx: Receiver<Ev>,
    pub(crate) explaining: HashMap<String, Cancel>,
    pub(crate) explain_queue: Option<(Vec<String>, usize)>,
    pub(crate) menu: Option<Menu>,
    pub(crate) pick: Option<Pick>,
    pub(crate) models: Option<Vec<String>>,
    pub(crate) show_settings: bool,
    pub(crate) show_help: bool,
    pub(crate) expanded: HashSet<String>,
    pub(crate) chat_open: bool,
    pub(crate) explorer_open: bool,
    pub(crate) hist_open: bool,
    /// Highlighting per card, with a fingerprint of the text it was computed from.
    /// (what it was computed from, the highlighting, when it was last used)
    pub(crate) spans: HashMap<u64, (SpanKey, Rc<Spans>, Instant)>,
    pub(crate) scrolls: HashMap<u64, ScrollHandle>,
    pub(crate) pending_bottom: HashSet<u64>,
    pub(crate) chat_lists: HashMap<u64, ChatList>,
    pub(crate) chat_view: gpui::Entity<ChatPanelView>,
    /// The cached chrome views, and the fingerprint of what each showed when last told to redraw.
    pub(crate) chrome: Vec<(Part, gpui::Entity<ChromeView>, u64)>,
    pub(crate) ui_font: &'static str,
    /// What the chat panel showed when last told to redraw.
    chat_key: u64,
    pub(crate) space: bool,
    pub(crate) toasts: Vec<(String, bool, Instant)>,
    pub(crate) char_ratio: f32,
    pub(crate) anim: HashMap<(u64, usize), f32>,
    pub(crate) fx: Fx,
    /// Open find bars, by card.
    pub(crate) finds: HashMap<u64, find::Find>,
    /// Cached text bitmaps, by (view, card, tile index). See `tiles.rs`.
    pub(crate) tiles: HashMap<tiles::Key, tiles::Cell>,
    /// The zoom each card (per view) was last drawn completely from.
    pub(crate) tile_shown: HashMap<(u64, u64), f32>,
    /// The zoom levels each card (per view) has drawn cells at, rebuilt every frame.
    pub(crate) tile_levels: HashMap<(u64, u64), Vec<f32>>,
    pub(crate) tile_pool: tiles::Pool,
    /// Finished tiles waiting for their turn to be uploaded.
    pub(crate) tile_inbox: std::collections::VecDeque<tiles::Done>,
    pub(crate) frame: u64,
    pub(crate) scale: f32,
    pub(crate) dead_tiles: Vec<Arc<gpui::RenderImage>>,
    /// Dot-grid tiles by dot spacing in device pixels, most recent last.
    pub(crate) dot_tiles: Vec<(u32, Arc<gpui::RenderImage>)>,
    pub(crate) ascii_glyphs: Rc<std::cell::RefCell<render::AsciiGlyphs>>,
    /// Current widths of the explorer and the global chat (they slide open and shut).
    pub(crate) side: (f32, f32),
    pub(crate) sel_at: Instant,
    pub(crate) started: Instant,
    pub(crate) focus: FocusHandle,
    pub(crate) win_size: (f32, f32),
    pub(crate) mouse: (f32, f32),
    mods: gpui::Modifiers,
    state_dirty: Option<Instant>,
    mtimes: HashMap<u64, SystemTime>,
    last_poll: Instant,
    inbox: Receiver<String>,
    /// The canvas state is written by its own thread, so a save never stalls a frame.
    saver: Sender<(Option<Doc>, String)>,
    /// False while only views moved since the last save: then the document isn't serialised again.
    doc_dirty: bool,
}

pub(crate) fn fl(p: Pixels) -> f32 { f32::from(p) }
fn now_secs() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) }
pub(crate) fn mono() -> &'static str {
    if cfg!(windows) { "Consolas" } else if cfg!(target_os = "macos") { "Menlo" } else { "DejaVu Sans Mono" }
}

/// The nearest zoom at which a line of code is a whole number of pixels tall, so rows are evenly
/// spaced when the view comes to rest. `fit` rounds down, to keep what was framed inside the view.
/// Far-out zooms, where cards show only their name, are left alone.
fn snap_zoom(z: f32, fit: bool) -> f32 {
    if z < 0.3 { return z; }
    let steps = z * LH;
    ((if fit { steps.floor() } else { steps.round() }).max(1.) / LH).clamp(MIN_Z, (MAX_Z * LH).floor() / LH)
}

fn leaf_ids(l: &Layout, out: &mut Vec<u64>) {
    match l { Layout::Pane(p) => out.push(*p), Layout::Split { children, .. } => children.iter().for_each(|c| leaf_ids(c, out)) }
}

fn collapse(l: Layout) -> Layout {
    match l {
        Layout::Split { horizontal, children, sizes } => {
            let mut children: Vec<Layout> = children.into_iter().map(collapse).collect();
            if children.len() == 1 { children.pop().unwrap() } else { Layout::Split { horizontal, children, sizes } }
        }
        p => p,
    }
}

fn split_leaf(l: &mut Layout, target: u64, new: u64, horiz: bool) -> bool {
    match l {
        Layout::Pane(p) if *p == target => {
            *l = Layout::Split { horizontal: horiz, children: vec![Layout::Pane(target), Layout::Pane(new)], sizes: vec![1.0, 1.0] };
            true
        }
        Layout::Pane(_) => false,
        Layout::Split { horizontal, children, sizes } => {
            if *horizontal == horiz {
                if let Some(i) = children.iter().position(|c| matches!(c, Layout::Pane(p) if *p == target)) {
                    let half = sizes[i] / 2.0;
                    sizes[i] = half;
                    children.insert(i + 1, Layout::Pane(new));
                    sizes.insert(i + 1, half);
                    return true;
                }
            }
            children.iter_mut().any(|c| split_leaf(c, target, new, horiz))
        }
    }
}

/// Removes a pane; returns the neighbour that takes over.
fn remove_leaf(l: &mut Layout, target: u64) -> Option<u64> {
    let Layout::Split { children, sizes, .. } = l else { return None };
    if let Some(i) = children.iter().position(|c| matches!(c, Layout::Pane(p) if *p == target)) {
        children.remove(i);
        sizes.remove(i);
        let mut ids = Vec::new();
        leaf_ids(&children[i.saturating_sub(1).min(children.len() - 1)], &mut ids);
        return ids.first().copied();
    }
    children.iter_mut().find_map(|c| remove_leaf(c, target))
}

impl CanvasView {
    pub fn new(root: PathBuf, file: Option<PathBuf>, inbox: Receiver<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let settings = Settings::load();
        let project = Arc::new(Project::new(root, settings.excluded()).expect("can't open the project folder"));
        let (tx, rx) = channel();
        // The saver keeps the document's JSON from the last time it changed. Panning and zooming only
        // change the small tail of the state file, so megabytes of file text aren't serialised again.
        let (saver, to_save) = channel::<(Option<Doc>, String)>();
        {
            let (project, tx) = (project.clone(), tx.clone());
            std::thread::spawn(move || {
                let mut doc_json = String::new();
                while let Ok((mut doc, mut rest)) = to_save.recv() {
                    while let Ok((d, r)) = to_save.try_recv() { if d.is_some() { doc = d; } rest = r; }
                    if let Some(d) = doc { match serde_json::to_string(&d) { Ok(j) => doc_json = j, Err(e) => { let _ = tx.send(Ev::SaveFailed(e.to_string())); continue; } } }
                    if doc_json.is_empty() { continue; }
                    if let Err(e) = project.save_state_json(&format!("{{\"doc\":{doc_json},{rest}}}")) { let _ = tx.send(Ev::SaveFailed(e.to_string())); }
                }
            });
        }
        cx.spawn(async move |this, cx| loop {
            // Quick while something is running (a chat answering, Explain, text being drawn), otherwise a
            // few times a second, which is plenty for files changing on disk and requests from Zed.
            let Ok(busy) = this.update(cx, |t, cx| { if t.pump() { cx.notify(); } t.working() }) else { break };
            cx.background_executor().timer(Duration::from_millis(if busy { 33 } else { 250 })).await;
        }).detach();
        let ts = window.text_system();
        let id = ts.resolve_font(&font(mono()));
        let char_ratio = ts.advance(id, px(100.), 'm').map(|s| fl(s.width) / 100.).unwrap_or(0.6);
        // The interface font. GPUI loads every face of a family into memory, and the usual default here,
        // Noto Sans, has 72 of them (about 35 MB); these families have a handful.
        let names = ts.all_font_names();
        let ui_font = ["Liberation Sans", "DejaVu Sans"].into_iter().find(|f| names.iter().any(|n| n == f)).unwrap_or(".SystemUIFont");
        let pv = PaneView { v: View { x: 60., y: 60., z: 1. }, t: View { x: 60., y: 60., z: 1. }, anchor: (0., 0.) };
        let tree = project.tree();
        let mut this = Self {
            project, settings, tree, doc: Doc { next_id: 1, ..Default::default() }, layout: Layout::Pane(1), views: HashMap::from([(1, pv)]), shared: true, active: 1,
            drag: None, live: None, selected: HashSet::new(), picked: None, hover_tab: None, doomed: Vec::new(), undo: Vec::new(), redo: Vec::new(),
            inputs: HashMap::new(), focus_input: None, editing: None, busy: HashMap::new(), tx, rx, explaining: HashMap::new(), explain_queue: None, menu: None, pick: None,
            models: None, show_settings: false, show_help: false, expanded: HashSet::new(), chat_open: true, explorer_open: true, hist_open: true, spans: HashMap::new(), scrolls: HashMap::new(),
            pending_bottom: HashSet::new(), chat_lists: HashMap::new(), chat_view: { let canvas = cx.weak_entity(); gpui::AppContext::new(cx, |_| ChatPanelView { canvas }) }, chat_key: 0, ui_font, chrome: [Part::Top, Part::Explorer, Part::Context].into_iter().map(|part| { let canvas = cx.weak_entity(); (part, gpui::AppContext::new(cx, |_| ChromeView { canvas, part }), 0) }).collect(), space: false, toasts: Vec::new(), char_ratio, anim: HashMap::new(), fx: Fx::default(), finds: HashMap::new(), tiles: HashMap::new(), tile_shown: HashMap::new(), tile_levels: HashMap::new(), tile_pool: tiles::Pool::new(mono()), tile_inbox: Default::default(), frame: 0, scale: 1., dead_tiles: Vec::new(), dot_tiles: Vec::new(), ascii_glyphs: Default::default(), side: (EXPLORER_W, CHAT_W), sel_at: Instant::now(), started: Instant::now(), focus, win_size: (1500., 950.), mouse: (0., 0.), mods: Default::default(),
            state_dirty: None, mtimes: HashMap::new(), last_poll: Instant::now(), inbox, saver, doc_dirty: true,
        };
        if let Some(s) = this.project.load_state() {
            this.doc = s.doc;
            this.layout = s.layout;
            this.shared = s.shared;
            this.active = s.active;
            this.views = s.views.into_iter().map(|(id, v)| (id, PaneView { v, t: v, anchor: (0., 0.) })).collect();
            let mut ids = Vec::new();
            leaf_ids(&this.layout, &mut ids);
            if ids.iter().any(|i| !this.views.contains_key(i)) { this.layout = Layout::Pane(1); this.views = HashMap::from([(1, pv)]); this.active = 1; }
            this.reload_from_disk();
        }
        if let Some(f) = file { this.open_abs(&f); }
        this
    }

    fn open_abs(&mut self, f: &std::path::Path) {
        let root = std::fs::canonicalize(&self.project.root).unwrap_or_default();
        let f = std::fs::canonicalize(f).unwrap_or_default();
        if let Ok(rel) = f.strip_prefix(root) { self.open(rel.to_string_lossy().replace('\\', "/"), None); }
    }

    // ---------- small helpers ----------
    pub(crate) fn toast(&mut self, s: impl Into<String>, err: bool) { self.toasts.push((s.into(), err, Instant::now())); }
    /// The document changed: save it soon.
    pub(crate) fn touch(&mut self) { self.state_dirty = Some(Instant::now()); self.doc_dirty = true; }
    pub(crate) fn snapshot(&mut self) { self.undo.push(self.doc.clone()); if self.undo.len() > 60 { self.undo.remove(0); } self.redo.clear(); }
    /// A view as it is drawn and hit-tested: its offset on whole pixels. Text can only sit on whole
    /// pixels vertically, so a canvas that glided by fractions would have its text jumping against its
    /// cards; this way everything moves together. The stored view keeps its exact value for the easing.
    pub(crate) fn pv(&self, pane: u64) -> PaneView {
        let mut p = self.views.get(&pane).copied().unwrap_or(self.views[&self.active]);
        p.v.x = p.v.x.round(); p.v.y = p.v.y.round();
        p
    }
    pub(crate) fn input(&mut self, k: Input) -> &mut TextBuffer { self.inputs.entry(k).or_default() }
    pub(crate) fn char_w(&self, z: f32) -> f32 { self.char_ratio * 12.5 * z }
    /// Glyph size for code at zoom `z`, in quarter-pixel steps: a zoom then reuses a bounded set of
    /// rasterised glyphs instead of adding a new set for every size it passes through. Positions are not
    /// stepped (see `paint_scaled`), so the text itself moves smoothly.
    pub(crate) fn code_px(&self, z: f32) -> f32 { ((12.5 * z * 4.).round() / 4.).max(0.25) }
    pub(crate) fn scroll_handle(&mut self, id: u64) -> ScrollHandle { self.scrolls.entry(id).or_default().clone() }

    pub(crate) fn canvas_area(&self) -> (f32, f32, f32, f32) {
        let (l, r) = self.side;
        (l, TOP, (self.win_size.0 - l - r).max(50.), (self.win_size.1 - TOP).max(50.))
    }

    pub(crate) fn pane_rects(&self) -> (Vec<Rect>, Vec<Sash>) {
        fn walk(l: &Layout, x: f32, y: f32, w: f32, h: f32, path: &mut Vec<usize>, p: &mut Vec<Rect>, s: &mut Vec<Sash>) {
            match l {
                Layout::Pane(id) => p.push(Rect { id: *id, x, y, w, h }),
                Layout::Split { horizontal, children, sizes } => {
                    let sum: f32 = sizes.iter().sum();
                    let span = if *horizontal { w } else { h };
                    let mut pos = 0.0;
                    for (i, c) in children.iter().enumerate() {
                        let len = span * sizes[i] / sum;
                        path.push(i);
                        if *horizontal { walk(c, x + pos, y, len, h, path, p, s); } else { walk(c, x, y + pos, w, len, path, p, s); }
                        path.pop();
                        pos += len;
                        if i + 1 < children.len() {
                            let (sx, sy, sw, sh) = if *horizontal { (x + pos - 2., y, 4., h) } else { (x, y + pos - 2., w, 4.) };
                            s.push(Sash { path: path.clone(), idx: i + 1, horizontal: *horizontal, x: sx, y: sy, w: sw, h: sh, span });
                        }
                    }
                }
            }
        }
        let (x, y, w, h) = self.canvas_area();
        let (mut p, mut s) = (Vec::new(), Vec::new());
        walk(&self.layout, x, y, w, h, &mut Vec::new(), &mut p, &mut s);
        (p, s)
    }

    pub(crate) fn pane_at(&self, sx: f32, sy: f32) -> Option<Rect> { self.pane_rects().0.into_iter().find(|r| sx >= r.x && sx < r.x + r.w && sy >= r.y && sy < r.y + r.h) }
    fn rect_of(&self, pane: u64) -> Rect { self.pane_rects().0.into_iter().find(|r| r.id == pane).unwrap_or(Rect { id: pane, x: 0., y: 0., w: 800., h: 600. }) }

    pub(crate) fn to_world(&self, pane: u64, sx: f32, sy: f32) -> (f32, f32) {
        let (r, v) = (self.rect_of(pane), self.pv(pane).v);
        ((sx - r.x - v.x) / v.z, (sy - r.y - v.y) / v.z)
    }

    pub(crate) fn in_pane(&self, c: &Card, pane: u64) -> bool { self.shared || c.pane == pane }

    pub(crate) fn card_height(&self, c: &Card) -> f32 { HEAD + PAD * 2. + c.lines.len() as f32 * LH }

    fn file_under(&self, pane: u64, wx: f32, wy: f32) -> Option<u64> {
        self.doc.cards.iter().rev().find(|c| c.is_file() && self.in_pane(c, pane) && wx >= c.x && wx <= c.x + c.w && wy >= c.y && wy <= c.y + self.card_height(c) + 40.).map(|c| c.id)
    }

    fn line_at(&self, c: &Card, pane: u64, sy: f32) -> usize {
        let (_, wy) = self.to_world(pane, 0., sy);
        (((wy - c.y - HEAD - PAD) / LH).floor().max(0.) as usize).min(c.lines.len().saturating_sub(1))
    }

    fn pane_for_card(&self, id: u64) -> u64 {
        if self.shared { return self.active; }
        self.doc.card(id).map_or(self.active, |c| c.pane)
    }

    // ---------- files ----------
    pub(crate) fn open(&mut self, rel: String, at: Option<(f32, f32, u64)>) {
        if let Some(id) = self.doc.file_by_path(&rel).map(|c| c.id) { if at.is_none() { self.focus_card(id); } return; }
        let Some(text) = self.project.read(&rel) else { self.toast(format!("Can't open {rel} (missing or binary)"), true); return };
        let lines = split_lines(&text);
        let longest = lines.iter().take(3000).map(|l| l.chars().count()).max().unwrap_or(0) as f32;
        let w = (longest * self.char_w(1.) + 90.).clamp(340., 980.);
        let (x, y, pane) = match at {
            Some(p) => p,
            None => {
                let r = self.rect_of(self.active);
                let (cx, cy) = self.to_world(self.active, r.x + r.w / 2., r.y + r.h / 2.);
                let p = self.find_spot(w, self.active, cx - w / 2., cy - 200.);
                (p.0, p.1, self.active)
            }
        };
        let id = self.doc.fresh_id();
        let lines = Lines::new(lines); // shared with `disk` until the card is edited
        self.doc.cards.push(Card { id, title: rel.clone(), lang: rel.clone(), path: Some(rel), pane, x, y, w, disk: lines.clone(), lines, color: 0, talk: Talk::default(), ref_of: None, target: None, sel: None });
        self.touch();
    }

    fn find_spot(&self, w: f32, pane: u64, px_: f32, py: f32) -> (f32, f32) {
        let rects: Vec<(f32, f32, f32, f32)> = self.doc.cards.iter().filter(|c| self.in_pane(c, pane)).map(|c| (c.x - 30., c.y - 30., c.x + c.w + 300., c.y + self.card_height(c) + 30.)).collect();
        let free = |x: f32, y: f32| !rects.iter().any(|r| x < r.2 && x + w > r.0 && y < r.3 && y + 400. > r.1);
        for ring in 0..40i32 {
            for k in 0..=ring {
                for (dx, dy) in [(ring, k), (k, ring), (-ring, k), (k, -ring), (ring, -k), (-k, ring), (-ring, -k), (-k, -ring)] {
                    let (x, y) = (px_ + dx as f32 * 120., py + dy as f32 * 120.);
                    if free(x, y) { return (x, y); }
                }
            }
        }
        (px_, py)
    }

    fn reload_from_disk(&mut self) {
        let files: Vec<(u64, String)> = self.doc.cards.iter().filter_map(|c| c.path.clone().map(|p| (c.id, p))).collect();
        for (id, rel) in files {
            if self.editing == Some(id) { continue; }
            let Some(m) = self.project.resolve(&rel, true).and_then(|a| std::fs::metadata(a).and_then(|m| m.modified()).ok()) else { continue };
            if self.mtimes.get(&id) == Some(&m) { continue; }
            self.mtimes.insert(id, m);
            if let Some(t) = self.project.read(&rel) { self.doc.apply_disk_text(id, &t); }
        }
    }

    pub(crate) fn save_file(&mut self, id: u64) {
        let Some(c) = self.doc.card(id) else { return };
        let Some(rel) = c.path.clone() else { return };
        let eol = self.project.read(&rel).map_or("\n", |t| eol_of(&t));
        let text = c.lines.join(eol);
        match self.project.write(&rel, &text) {
            Ok(()) => {
                let c = self.doc.card_mut(id).unwrap();
                c.disk = c.lines.clone();
                self.mtimes.remove(&id);
                self.toast(format!("Saved {rel}"), false);
            }
            Err(e) => self.toast(e, true),
        }
    }
    pub(crate) fn save_all(&mut self) { let ids: Vec<u64> = self.doc.cards.iter().filter(|c| c.dirty()).map(|c| c.id).collect(); ids.into_iter().for_each(|i| self.save_file(i)); }
    pub(crate) fn any_dirty(&self) -> bool { self.doc.cards.iter().any(Card::dirty) }

    pub(crate) fn close_card(&mut self, id: u64) { self.snapshot(); if self.editing == Some(id) { self.editing = None; self.focus_input = None; } self.doc.remove_card(id); self.selected.remove(&id); self.touch(); }

    fn persist(&mut self) {
        self.state_dirty = None;
        let views: Vec<(u64, View)> = self.views.iter().map(|(k, p)| (*k, p.t)).collect();
        let json = |v: serde_json::Result<String>| v.unwrap_or_else(|_| "null".into());
        let rest = format!("\"layout\":{},\"views\":{},\"shared\":{},\"active\":{}", json(serde_json::to_string(&self.layout)), json(serde_json::to_string(&views)), self.shared, self.active);
        // cloning the document is cheap: card text is shared, not copied
        let doc = std::mem::take(&mut self.doc_dirty).then(|| self.doc.for_saving());
        let _ = self.saver.send((doc, rest));
    }

    pub(crate) fn open_in_editor(&mut self, id: u64, line: usize) {
        let Some(abs) = self.doc.card(id).and_then(|c| c.path.as_ref()).and_then(|p| self.project.resolve(p, true)) else { return };
        let target = format!("{}:{line}", abs.display());
        let mut cands: Vec<PathBuf> = Vec::new();
        if !self.settings.editor_command.trim().is_empty() { cands.push(self.settings.editor_command.trim().into()); }
        else {
            if let Ok(l) = std::env::var("LOCALAPPDATA") { cands.push(PathBuf::from(l).join("Programs/Zed/bin/Zed.exe")); }
            cands.push("zed".into()); cands.push("zeditor".into());
        }
        if !cands.iter().any(|c| std::process::Command::new(c).arg(&target).spawn().is_ok()) { self.toast("Couldn't start Zed. Set its path in Settings.", true); }
    }

    // ---------- view stepping ----------
    pub(crate) fn step(&mut self) {
        let mut moved = false;
        let (kz, kp) = (self.fx.step(0.22), self.fx.step(0.3));
        for p in self.views.values_mut() {
            let (t, a) = (p.t, p.anchor);
            let v = &mut p.v;
            if (t.z - v.z).abs() > 1e-4 {
                let nz = if (t.z / v.z - 1.).abs() < 0.005 { t.z } else { v.z * (t.z / v.z).powf(kz) };
                let r = nz / v.z;
                v.x = a.0 - (a.0 - v.x) * r; v.y = a.1 - (a.1 - v.y) * r; v.z = nz;
                moved = true;
            }
            let dx = a.0 - ((a.0 - t.x) / t.z) * v.z;
            let dy = a.1 - ((a.1 - t.y) / t.z) * v.z;
            if (dx - v.x).abs() > 0.3 || (dy - v.y).abs() > 0.3 { v.x += (dx - v.x) * kp; v.y += (dy - v.y) * kp; moved = true; } else { v.x = dx; v.y = dy; }
        }
        if moved {
            self.fx.want_frame = true;
            self.state_dirty = Some(Instant::now()); // the view moved; the document didn't
            if self.drag.is_some() { let (x, y) = self.mouse; let m = self.mods; self.pointer_moved(x, y, m); }
        }
    }

    pub(crate) fn on_wheel(&mut self, pane: u64, e: &ScrollWheelEvent, cx: &mut Context<Self>) {
        self.menu = None;
        self.active = pane;
        // A wheel notch arrives as three "lines"; 40 px each makes it the 120 px a browser scrolls,
        // so the wheel covers the same distance (and zooms by the same amount) as the VS Code version.
        let notched = matches!(e.delta, gpui::ScrollDelta::Lines(_));
        let d = e.delta.pixel_delta(px(40.));
        let (dx, dy) = (fl(d.x), fl(d.y));
        let r = self.rect_of(pane);
        let shift_lines = matches!(self.drag, Some(Drag::ShiftLines { .. })) || self.live.as_ref().is_some_and(|l| l.lock);
        let p = self.views.get_mut(&pane).unwrap();
        if e.modifiers.control || e.modifiers.platform {
            p.anchor = (fl(e.position.x) - r.x, fl(e.position.y) - r.y);
            let nz = (p.t.z * (dy * 0.0016).exp()).clamp(MIN_Z, MAX_Z);
            // a wheel lands on a zoom with a whole-pixel line pitch; a touchpad pinch stays continuous
            let nz = if notched { let s = snap_zoom(nz, false); if s == p.t.z { snap_zoom(p.t.z + dy.signum() / LH, false) } else { s } } else { nz };
            let (a, t) = (p.anchor, &mut p.t);
            t.x = a.0 - (a.0 - t.x) * (nz / t.z); t.y = a.1 - (a.1 - t.y) * (nz / t.z); t.z = nz;
        } else if shift_lines {
            p.t.y += if dy != 0. { dy } else { dx };
        } else if e.modifiers.shift {
            p.t.x += if dy != 0. { dy } else { dx };
        } else {
            p.t.y += dy; p.t.x += dx;
        }
        cx.notify();
    }

    pub(crate) fn frame(&mut self, pane: u64, x0: f32, y0: f32, x1: f32, y1: f32, max_z: f32) {
        let r = self.rect_of(pane);
        let z = ((r.w - 120.) / (x1 - x0).max(1.)).min((r.h - 120.) / (y1 - y0).max(1.)).clamp(MIN_Z, max_z);
        let z = snap_zoom(z, true).min(max_z);
        let p = self.views.get_mut(&pane).unwrap();
        p.t = View { z, x: r.w / 2. - (x0 + x1) / 2. * z, y: r.h / 2. - (y0 + y1) / 2. * z };
        p.anchor = (r.w / 2., r.h / 2.);
    }

    pub(crate) fn fit(&mut self, pane: u64) {
        let cs: Vec<(f32, f32, f32, f32)> = self.doc.cards.iter().filter(|c| self.in_pane(c, pane) && (self.selected.is_empty() || self.selected.contains(&c.id))).map(|c| (c.x, c.y, c.x + c.w, c.y + self.card_height(c))).collect();
        if cs.is_empty() { return; }
        let b = cs.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |a, c| (a.0.min(c.0), a.1.min(c.1), a.2.max(c.2), a.3.max(c.3)));
        self.frame(pane, b.0, b.1, b.2 + 260., b.3, 1.);
    }

    pub(crate) fn focus_card(&mut self, id: u64) {
        let (card, line) = match self.doc.region(id) { Some(r) => (r.file, Some(r.start)), None => (id, None) };
        let Some(c) = self.doc.card(card).cloned() else { return };
        let pane = if self.shared { self.active } else { c.pane };
        self.active = pane;
        let y = c.y + line.map_or(0., |l| HEAD + PAD + l as f32 * LH);
        self.frame(pane, c.x - 40., y - 40., c.x + c.w + 400., y + self.card_height(&c).min(800.), 1.);
        if line.is_some() { self.hover_tab = Some(id); }
    }

    pub(crate) fn zoom_keys(&mut self, f: f32) {
        let r = self.rect_of(self.active);
        let p = self.views.get_mut(&self.active).unwrap();
        p.anchor = (r.w / 2., r.h / 2.);
        let nz = if f == 0. { 1. } else { snap_zoom((p.t.z * f).clamp(MIN_Z, MAX_Z), false) };
        let (a, t) = (p.anchor, &mut p.t);
        t.x = a.0 - (a.0 - t.x) * (nz / t.z); t.y = a.1 - (a.1 - t.y) * (nz / t.z); t.z = nz;
    }

    // ---------- panes ----------
    pub(crate) fn split(&mut self, horizontal: bool) {
        let id = self.doc.fresh_id();
        let pv = self.pv(self.active);
        self.views.insert(id, pv);
        split_leaf(&mut self.layout, self.active, id, horizontal);
        self.active = id;
        self.touch();
    }

    pub(crate) fn close_pane(&mut self, pane: u64) {
        if self.views.len() < 2 { return; }
        let Some(next) = remove_leaf(&mut self.layout, pane) else { return };
        self.layout = collapse(self.layout.clone());
        self.views.remove(&pane);
        if !self.shared {
            let right = self.doc.cards.iter().filter(|c| c.pane == next).map(|c| c.x + c.w).fold(f32::MIN, f32::max);
            let left = self.doc.cards.iter().filter(|c| c.pane == pane).map(|c| c.x).fold(f32::MAX, f32::min);
            let dx = if right > f32::MIN && left < f32::MAX { right + 400. - left } else { 0. };
            for c in self.doc.cards.iter_mut().filter(|c| c.pane == pane) { c.pane = next; c.x += dx; }
        }
        if self.active == pane { self.active = next; }
        self.touch();
    }

    pub(crate) fn set_shared(&mut self, on: bool) {
        if on == self.shared { return; }
        if on {
            let mut right = self.doc.cards.iter().filter(|c| c.pane == self.active).map(|c| c.x + c.w).fold(f32::MIN, f32::max);
            let others: Vec<u64> = self.views.keys().copied().filter(|p| *p != self.active).collect();
            for p in others {
                let left = self.doc.cards.iter().filter(|c| c.pane == p).map(|c| c.x).fold(f32::MAX, f32::min);
                if left == f32::MAX { continue; }
                let dx = if right > f32::MIN { right + 400. - left } else { 0. };
                let active = self.active;
                for c in self.doc.cards.iter_mut().filter(|c| c.pane == p) { c.pane = active; c.x += dx; right = right.max(c.x + c.w); }
            }
        } else {
            let a = self.active;
            self.doc.cards.iter_mut().for_each(|c| c.pane = a);
        }
        self.shared = on;
        self.touch();
    }

    // ---------- pointer ----------
    pub(crate) fn pane_mouse_down(&mut self, pane: u64, e: &MouseDownEvent, cx: &mut Context<Self>) {
        self.active = pane;
        self.menu = None;
        let (sx, sy) = (fl(e.position.x), fl(e.position.y));
        self.mouse = (sx, sy);
        if self.finish_pick(pane, sx, sy) { cx.notify(); return; }
        self.exit_edit();
        self.focus_input = None;
        self.clear_line_sel();
        let panning = e.button == gpui::MouseButton::Middle || self.space;
        let v = self.pv(pane).v;
        if panning {
            self.drag = Some(Drag::Pan { pane, sx, sy, ox: v.x, oy: v.y });
        } else if e.click_count >= 2 {
            let (x, y) = self.to_world(pane, sx, sy);
            let id = self.doc.add_floating("New snippet".into(), vec![String::new()], "x.ts".into(), x, y, 400., pane, None);
            self.enter_edit(id, 0, 0);
        } else {
            let keep = e.modifiers.control || e.modifiers.shift;
            let base = if keep { self.selected.clone() } else { HashSet::new() };
            if !keep { self.selected.clear(); self.picked = None; }
            self.drag = Some(Drag::Marquee { pane, sx, sy, cx: sx, cy: sy, base, moved: false });
        }
        cx.notify();
    }

    fn clear_line_sel(&mut self) { for c in &mut self.doc.cards { c.sel = None; } }

    /// Clicking a card takes the keyboard away from chat boxes and finishes any other card's edit.
    fn release_focus(&mut self, keep_editing: Option<u64>) {
        if self.editing.is_some() && self.editing != keep_editing { self.exit_edit(); }
        if !matches!(self.focus_input, Some(Input::Editor(_))) { self.focus_input = None; }
    }

    pub(crate) fn start_move(&mut self, id: u64, e: &MouseDownEvent, pane: u64, cx: &mut Context<Self>) {
        if self.live.is_some() { return; }
        self.release_focus(Some(id));
        let (sx, sy) = (fl(e.position.x), fl(e.position.y));
        if e.modifiers.control || e.modifiers.shift { if !self.selected.remove(&id) { self.selected.insert(id); } }
        else if !self.selected.contains(&id) { self.selected = HashSet::from([id]); self.picked = None; }
        self.clear_line_sel();
        let (wx, wy) = self.to_world(pane, sx, sy);
        let ids: Vec<u64> = if self.selected.contains(&id) { self.selected.iter().copied().filter(|i| self.doc.card(*i).is_some()).collect() } else { vec![id] };
        let items: Vec<MoveItem> = ids.iter().filter_map(|i| self.doc.card(*i)).map(|c| MoveItem { id: c.id, dx: wx - c.x, dy: wy - c.y, ox: c.x, oy: c.y, pane: c.pane }).collect();
        let single = (items.len() == 1 && !self.doc.card(id).is_some_and(Card::is_file)).then_some(id);
        for it in &items {
            if let Some(i) = self.doc.cards.iter().position(|c| c.id == it.id) { let c = self.doc.cards.remove(i); self.doc.cards.push(c); }
        }
        self.drag = Some(Drag::Move { items, sx, sy, moved: false, single });
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn file_body_down(&mut self, id: u64, pane: u64, e: &MouseDownEvent, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.active = pane;
        self.menu = None;
        let (sx, sy) = (fl(e.position.x), fl(e.position.y));
        self.mouse = (sx, sy);
        if self.finish_pick(pane, sx, sy) { cx.notify(); return; }
        if self.space { self.start_move(id, e, pane, cx); return; }
        let Some(c) = self.doc.card(id).cloned() else { return };
        let line = self.line_at(&c, pane, sy);
        if self.editing == Some(id) {
            let col = self.col_at(&c, pane, sx, line, id);
            let b = self.input(Input::Editor(id));
            b.set_cursor(line, col, e.modifiers.shift);
            if !e.modifiers.shift { b.anchor = Some(b.cur); }
            self.focus_input = Some(Input::Editor(id));
            self.drag = Some(Drag::EditSelect { id });
            cx.notify();
            return;
        }
        self.exit_edit();
        self.focus_input = None;
        if e.click_count >= 2 {
            let col = self.col_at(&c, pane, sx, line, id);
            self.enter_edit(id, line, col);
            cx.notify();
            return;
        }
        if e.modifiers.shift {
            if let Some((a, b)) = c.sel_range().filter(|(a, b)| line >= *a && line <= *b) {
                self.drag = Some(Drag::ShiftLines { id, a, b, sy, wy: self.to_world(pane, 0., sy).1, pane });
                cx.notify();
                return;
            }
            // Shift on a snippet's lines: a drag grabs it (inside the file or out of it); a click is the usual Shift+click, done on release.
            let held = self.doc.regions_of(id).into_iter().filter(|r| line >= r.start && line <= r.end).min_by_key(|r| r.end - r.start);
            if let Some(r) = held { self.start_region(r.id, Via::Shift, e, pane, cx); return; }
        }
        if e.modifiers.control || e.modifiers.shift {
            if !self.selected.remove(&id) { self.selected.insert(id); }
            cx.notify();
            return;
        }
        let hit = self.doc.regions_of(id).into_iter().filter(|r| self.selected.contains(&r.id) && line >= r.start && line <= r.end).min_by_key(|r| r.end - r.start);
        if let Some(r) = hit { self.start_region(r.id, Via::Lines, e, pane, cx); return; }
        self.clear_line_sel();
        self.selected.clear();
        self.picked = None;
        self.doc.card_mut(id).unwrap().sel = Some((line, line));
        self.drag = Some(Drag::Select { id });
        cx.notify();
    }

    fn col_at(&self, c: &Card, pane: u64, sx: f32, line: usize, id: u64) -> usize {
        let (wx, _) = self.to_world(pane, sx, 0.);
        let gutter = self.gutter(c.lines.len());
        let vis = ((wx - c.x - gutter) / self.char_w(1.)).round().max(0.) as usize;
        let l = self.inputs.get(&Input::Editor(id)).and_then(|b| b.lines.get(line)).or(c.lines.get(line));
        l.map_or(vis, |l| visual_to_chars(l, vis))
    }

    pub(crate) fn gutter(&self, n: usize) -> f32 { n.to_string().len() as f32 * self.char_w(1.) + 22. }

    pub(crate) fn start_region(&mut self, id: u64, via: Via, e: &MouseDownEvent, pane: u64, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.release_focus(None);
        let (sx, sy) = (fl(e.position.x), fl(e.position.y));
        let (wx, wy) = self.to_world(pane, sx, sy);
        if via == Via::Bar { self.picked = None; self.selected = HashSet::from([id]); }
        self.drag = Some(Drag::Region { id, via, sx, sy, wx, wy, pane });
        cx.notify();
    }

    pub(crate) fn start_edge(&mut self, id: u64, top: bool, e: &MouseDownEvent, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.release_focus(None);
        let Some(r) = self.doc.region(id) else { return };
        self.drag = Some(Drag::Edge { id, top, orig: (r.start, r.end), moved: false, sx: fl(e.position.x), sy: fl(e.position.y) });
        cx.notify();
    }

    pub(crate) fn on_move(&mut self, e: &MouseMoveEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let (x, y) = (fl(e.position.x), fl(e.position.y));
        self.mouse = (x, y);
        self.mods = e.modifiers;
        if let Some(p) = &self.pick {
            let file = p.file;
            let over = self.pane_at(x, y).and_then(|r| {
                let (wx, wy) = self.to_world(r.id, x, y);
                let f = self.file_under(r.id, wx, wy).filter(|f| file.is_none_or(|t| t == *f))?;
                self.pick_spot(f, wy, &p.snippets)
            });
            self.pick.as_mut().unwrap().over = over;
            cx.notify();
        }
        if self.drag.is_some() { self.pointer_moved(x, y, e.modifiers); cx.notify(); }
    }

    fn pointer_moved(&mut self, x: f32, y: f32, m: gpui::Modifiers) {
        let Some(drag) = self.drag.take() else { return };
        let keep = match drag {
            Drag::Pan { pane, sx, sy, ox, oy } => {
                let p = self.views.get_mut(&pane).unwrap();
                p.v.x = ox + x - sx; p.v.y = oy + y - sy;
                p.t = p.v; p.anchor = (0., 0.);
                drag
            }
            Drag::Marquee { pane, sx, sy, ref base, moved, .. } if !moved && (x - sx).hypot(y - sy) < 4. => Drag::Marquee { pane, sx, sy, cx: x, cy: y, base: base.clone(), moved },
            Drag::Marquee { pane, sx, sy, ref base, .. } => {
                let (a, b) = (self.to_world(pane, sx.min(x), sy.min(y)), self.to_world(pane, sx.max(x), sy.max(y)));
                let mut sel = base.clone();
                for c in self.doc.cards.iter().filter(|c| self.in_pane(c, pane)) {
                    if c.x < b.0 && c.x + c.w > a.0 && c.y < b.1 && c.y + self.card_height(c) > a.1 { sel.insert(c.id); }
                }
                self.selected = sel;
                Drag::Marquee { pane, sx, sy, cx: x, cy: y, base: base.clone(), moved: true }
            }
            Drag::Move { items, sx, sy, mut moved, single } => {
                if !moved && (x - sx).hypot(y - sy) < 3. { Drag::Move { items, sx, sy, moved, single } }
                else {
                    if !moved && single.is_some() && self.live.is_none() {
                        let c = self.doc.card(single.unwrap()).unwrap();
                        self.fx.forget(Tw::Ghost);
                        self.live = Some(Live { n: c.lines.len(), src: None, region: None, over: None, at: 0, ref_mode: false, lock: false, ghost_card: Some(c.id), lines: c.lines.to_vec(), lang: c.lang.clone(), title: c.title.clone(), color: c.color, gx: c.x, gy: c.y, gw: c.w, pane: c.pane });
                    }
                    moved = true;
                    let pane = self.pane_at(x, y).map_or(self.active, |r| r.id);
                    if pane != self.active { self.active = pane; }
                    let (wx, wy) = self.to_world(pane, x, y);
                    for it in &items {
                        let shared = self.shared;
                        if let Some(c) = self.doc.card_mut(it.id) { c.x = wx - it.dx; c.y = wy - it.dy; if !shared { c.pane = pane; } }
                    }
                    if let Some(id) = single {
                        let c = self.doc.card(id).unwrap();
                        let (gx, gy) = (c.x, c.y);
                        self.live_hover(pane, gx, gy, wy, m.alt);
                    }
                    Drag::Move { items, sx, sy, moved, single }
                }
            }
            Drag::Select { id } => {
                let pane = self.pane_for_card(id);
                if let Some(c) = self.doc.card(id).cloned() { let l = self.line_at(&c, pane, y); if let Some(s) = &mut self.doc.card_mut(id).unwrap().sel { s.1 = l; } }
                drag
            }
            Drag::EditSelect { id } => {
                let pane = self.pane_for_card(id);
                if let Some(c) = self.doc.card(id).cloned() {
                    let l = self.line_at(&c, pane, y).min(self.input(Input::Editor(id)).lines.len() - 1);
                    let col = self.col_at(&c, pane, x, l, id);
                    self.input(Input::Editor(id)).set_cursor(l, col, true);
                }
                drag
            }
            Drag::Resize { id, sx, ow, z } => { if let Some(c) = self.doc.card_mut(id) { c.w = (ow + (x - sx) / z).max(240.); } drag }
            Drag::Region { id, via, sx, sy, wx, wy, pane } => {
                if self.live.is_none() {
                    if (x - sx).hypot(y - sy) >= 5. { self.begin_region_live(id, pane); }
                }
                if let Some(mut l) = self.live.take() {
                    // a Shift drag stays in its own file and view: it only moves up and down
                    let p = if l.lock { pane } else { self.pane_at(x, y).map_or(l.pane, |r| r.id) };
                    let (px_, py) = self.to_world(p, x, y);
                    if l.gw == 0. { l.gw = 400.; }
                    if let Some(f) = l.src.and_then(|s| self.doc.card(s.0)) {
                        let top = f.y + HEAD + PAD + l.src.unwrap().1 as f32 * LH;
                        l.gx = if l.lock { f.x } else { f.x + (px_ - wx) };
                        l.gy = top + (py - wy);
                    }
                    l.pane = p;
                    let alt = m.alt && !l.lock;
                    if l.ref_mode != alt { l.ref_mode = alt; l.over = None; }
                    self.live = Some(l);
                    let (gx, gy) = { let l = self.live.as_ref().unwrap(); (l.gx, l.gy - HEAD - PAD) };
                    self.live_hover(p, gx, gy, py, false);
                }
                Drag::Region { id, via, sx, sy, wx, wy, pane }
            }
            Drag::Edge { id, top, orig, moved, sx, sy } => {
                if !moved && (x - sx).hypot(y - sy) < 4. { Drag::Edge { id, top, orig, moved, sx, sy } }
                else {
                    if !moved { self.snapshot(); self.selected = HashSet::from([id]); self.picked = None; }
                    if let Some(r) = self.doc.region(id).cloned() {
                        let pane = self.pane_for_card(r.file);
                        let c = self.doc.card(r.file).unwrap().clone();
                        let line = self.line_at(&c, pane, y);
                        let m = self.doc.region_mut(id).unwrap();
                        if top { m.start = line.min(m.end); } else { m.end = line.max(m.start); }
                        self.doomed = self.doc.resize_victims(id);
                    }
                    Drag::Edge { id, top, orig, moved: true, sx, sy }
                }
            }
            Drag::ShiftLines { id, a, b, sy, wy, pane } => {
                if self.live.is_none() && (y - sy).abs() >= 4. {
                    let c = self.doc.card(id).unwrap().clone();
                    self.fx.forget(Tw::Ghost);
                    self.live = Some(Live { n: b - a + 1, src: Some((id, a, b)), region: None, over: Some(id), at: a, ref_mode: false, lock: true, ghost_card: None, lines: c.lines[a..=b].to_vec(), lang: c.lang.clone(), title: String::new(), color: 0x4c9aff, gx: c.x, gy: c.y + HEAD + PAD + a as f32 * LH, gw: c.w, pane });
                    self.doc.card_mut(id).unwrap().sel = None;
                }
                let py = { let r = self.rect_of(pane); let v = self.pv(pane).v; (y - r.y - v.y) / v.z };
                let c_y = self.doc.card(id).unwrap().y;
                if let Some(l) = &mut self.live {
                    l.gy = c_y + HEAD + PAD + a as f32 * LH + (py - wy);
                    let gy = l.gy - HEAD - PAD;
                    let gx = l.gx;
                    self.live_hover(pane, gx, gy, py, false);
                }
                Drag::ShiftLines { id, a, b, sy, wy, pane }
            }
            Drag::Sash { sash, start, a0, pair, sum } => {
                let pos = if sash.horizontal { x } else { y };
                let a = (a0 + (pos - start) / sash.span * sum).clamp(pair * 0.08, pair * 0.92);
                let mut node = &mut self.layout;
                for i in &sash.path { if let Layout::Split { children, .. } = node { node = &mut children[*i]; } }
                if let Layout::Split { sizes, .. } = node { sizes[sash.idx - 1] = a; sizes[sash.idx] = pair - a; }
                Drag::Sash { sash, start, a0, pair, sum }
            }
            Drag::Explorer { path, sx, sy, moved } => Drag::Explorer { moved: moved || (x - sx).hypot(y - sy) > 4., path, sx, sy },
        };
        self.drag = Some(keep);
    }

    fn begin_region_live(&mut self, id: u64, pane: u64) {
        let Some(r) = self.doc.region(id).cloned() else { return };
        let Some(f) = self.doc.card(r.file).cloned() else { return };
        let lines = self.doc.region_lines(&r);
        self.clear_line_sel();
        self.fx.forget(Tw::Ghost);
        self.live = Some(Live { n: lines.len(), src: Some((r.file, r.start, r.end)), region: Some(id), over: Some(r.file), at: r.start, ref_mode: false, lock: false, ghost_card: None, lines, lang: f.lang.clone(), title: r.title.clone(), color: r.color, gx: f.x, gy: f.y + HEAD + PAD + r.start as f32 * LH, gw: f.w, pane });
    }

    /// `gx`/`gy` are the dragged block's card origin in world units; `py` is the pointer's world y.
    fn live_hover(&mut self, pane: u64, gx: f32, gy: f32, py: f32, alt_blocks: bool) {
        let Some(l) = &self.live else { return };
        let over = if l.lock { l.src.map(|s| s.0) } else if alt_blocks { None } else { self.file_under(pane, gx + (l.gw * 0.3).min(160.), py) };
        let mut at = 0;
        if let Some(f) = over {
            let c = self.doc.card(f).unwrap();
            let lifted = l.src.is_some_and(|s| s.0 == f) && !l.ref_mode;
            let remaining = c.lines.len() - if lifted { l.n } else { 0 };
            at = (((gy + HEAD + PAD - (c.y + HEAD + PAD)) / LH).round().max(0.) as usize).min(remaining);
        }
        let l = self.live.as_mut().unwrap();
        l.over = over;
        l.at = at;
    }

    pub(crate) fn on_up(&mut self, e: &MouseUpEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        let (x, y) = (fl(e.position.x), fl(e.position.y));
        let alt = e.modifiers.alt;
        match drag {
            Drag::Move { items, moved, single, .. } => {
                if let (Some(id), true) = (single, moved) {
                    if let Some(l) = self.live.take() {
                        if let Some(f) = l.over {
                            let it = &items[0];
                            let cur = { let c = self.doc.card_mut(id).unwrap(); let cur = (c.x, c.y, c.pane); c.x = it.ox; c.y = it.oy; c.pane = it.pane; cur };
                            self.snapshot();
                            let c = self.doc.card_mut(id).unwrap(); c.x = cur.0; c.y = cur.1; c.pane = cur.2;
                            if self.doc.merge(id, f, l.at, 0) { self.landed(f, l.at, l.n, cur.0, id); }
                            self.selected = HashSet::from([id]);
                            self.picked = None;
                        }
                    }
                }
                self.live = None;
            }
            Drag::Region { id, via, .. } => match self.live.take() {
                None => match via {
                    Via::Tab => self.toggle_chat(id),
                    Via::Lines | Via::Bar => { self.selected = HashSet::from([id]); self.picked = Some(id); }
                    // Shift+click: the selection changes on release, never on a drag
                    Via::Shift => if let Some(f) = self.doc.region(id).map(|r| r.file) { if !self.selected.remove(&f) { self.selected.insert(f); } },
                },
                Some(l) => { let alt = alt && !l.lock; self.drop_region(id, l, alt) }
            },
            Drag::Edge { id, moved, orig, .. } => {
                let victims = std::mem::take(&mut self.doomed);
                if !moved { self.selected = HashSet::from([id]); self.picked = Some(id); }
                else if self.doc.region(id).is_some_and(|r| (r.start, r.end) == orig) { self.undo.pop(); }
                else {
                    if !victims.is_empty() { self.toast(format!("Dissolved {} overlapping snippet(s); the code stays", victims.len()), false); }
                    self.doc.dissolve(&victims);
                    self.doc.sync_refs();
                }
            }
            Drag::ShiftLines { id, a, b, .. } => match self.live.take() {
                None => {}
                Some(l) => {
                    let n = b - a + 1;
                    if l.at != a { self.snapshot(); self.doc.move_lines(id, a, b, l.at); }
                    self.fx.line(id, l.at, n, LineKind::Settle, 0.);
                    self.doc.card_mut(id).unwrap().sel = Some((l.at, l.at + n - 1));
                    self.sel_at = Instant::now();
                }
            },
            Drag::Explorer { path, moved, .. } => {
                if !moved { self.open(path, None); }
                else if let Some(r) = self.pane_at(x, y) {
                    let (wx, wy) = self.to_world(r.id, x, y);
                    match self.doc.file_by_path(&path).map(|c| c.id) {
                        Some(id) => { if let Some(c) = self.doc.card_mut(id) { c.x = wx - 40.; c.y = wy - 16.; c.pane = r.id; } }
                        None => self.open(path, Some((wx - 40., wy - 16., r.id))),
                    }
                }
            }
            Drag::Select { .. } => self.sel_at = Instant::now(),
            _ => {}
        }
        self.anim.clear();
        self.fx.forget(Tw::Ghost);
        self.touch();
        cx.notify();
    }

    /// A dragged block was dropped into `file` at `at`: its breathing room closes and its tabs fade in.
    fn landed(&mut self, file: u64, at: usize, n: usize, gx: f32, region: u64) {
        let dx = self.doc.card(file).map_or(0., |f| gx - f.x);
        self.fx.line(file, at, n, LineKind::Settle, 0.);
        if let Some(l) = self.fx.lines.last_mut() { l.dx = dx; }
        self.fx.fresh.insert(region, Instant::now());
    }

    fn drop_region(&mut self, id: u64, l: Live, alt: bool) {
        let Some(r) = self.doc.region(id).cloned() else { return };
        match (l.over, alt) {
            (Some(f), true) => {
                self.snapshot();
                self.doc.splice(f, l.at, 0, l.lines.clone());
                let c = self.doc.create_region(f, l.at, l.at + l.n - 1, Some(r.title), false, vec![]);
                self.landed(f, l.at, l.n, l.gx, c);
                self.selected = HashSet::from([c]);
            }
            (Some(f), false) => {
                let stayed = f == r.file && l.at == r.start;
                if !stayed { self.snapshot(); self.doc.move_region(id, f, l.at); }
                self.landed(f, l.at, l.n, l.gx, id);
                if stayed { self.fx.fresh.remove(&id); }
                self.selected = HashSet::from([id]);
            }
            (None, true) => { self.snapshot(); self.doc.make_reference(id, l.gx, l.gy - HEAD - PAD, l.pane); }
            (None, false) => {
                self.snapshot();
                self.doc.detach_region(id, l.gx, l.gy - HEAD - PAD, l.pane);
                self.selected = HashSet::from([id]);
            }
        }
        self.picked = None;
    }

    // ---------- merge / pick ----------
    pub(crate) fn start_pick(&mut self, snippets: Vec<u64>, file: Option<u64>) {
        self.pick = Some(Pick { snippets, file, over: None });
        self.toast("Click a line to merge. Esc cancels.", false);
    }

    fn finish_pick(&mut self, _pane: u64, _sx: f32, _sy: f32) -> bool {
        let Some(p) = self.pick.take() else { return false };
        if let Some((f, at, del)) = p.over {
            self.snapshot();
            let mut list = p.snippets;
            // bottom-most first at the same point, so they end up in top-to-bottom order
            list.sort_by(|a, b| { let y = |i: &u64| self.doc.card(*i).map_or(0., |c| c.y); y(b).partial_cmp(&y(a)).unwrap() });
            for (i, s) in list.into_iter().enumerate() { self.merge_fx(s, f, at, if i == 0 { del } else { 0 }); }
            self.clear_line_sel();
            self.touch();
        }
        true
    }

    /// Where a picked snippet would land in `file` with the pointer at world y `wy`: (line, lines replaced).
    /// Over the file's line selection, or over the range the LLM suggested for a single snippet, it replaces.
    fn pick_spot(&self, file: u64, wy: f32, snippets: &[u64]) -> Option<(u64, usize, usize)> {
        let c = self.doc.card(file)?;
        let pos = (wy - c.y - HEAD - PAD) / LH;
        let line = (pos.floor().max(0.) as usize).min(c.lines.len().saturating_sub(1));
        let mut rep = c.sel_range().filter(|(a, b)| line >= *a && line <= *b);
        if rep.is_none() && snippets.len() == 1 {
            let target = self.doc.card(snippets[0]).and_then(|s| s.target.as_ref()).filter(|t| Some(&t.file) == c.path.as_ref()).and_then(|t| t.replace);
            if let Some((a, b)) = target {
                let (a, b) = (a.saturating_sub(1).min(c.lines.len() - 1), b.saturating_sub(1).min(c.lines.len() - 1));
                if line + 1 >= a && line <= b + 1 && b >= a { rep = Some((a, b)); }
            }
        }
        Some(match rep { Some((a, b)) => (file, a, b - a + 1), None => (file, (pos.round().max(0.) as usize).min(c.lines.len()), 0) })
    }

    // ---------- moves that are shown as lines flying between cards ----------
    fn code_origin(&self, c: &Card, line: usize) -> (f32, f32) { (c.x + self.gutter(c.lines.len()), c.y + HEAD + PAD + line as f32 * LH) }

    /// Sends copies of `lines` flying from one spot to another; returns how long the flight takes (ms).
    fn fly(&mut self, pane: u64, lines: &[String], lang: &str, from: (f32, f32), to: (f32, f32), color: u32) -> f32 {
        let lines = &lines[..lines.len().min(140)];
        let hl = highlight(lines, lang);
        let rows = lines.iter().enumerate().map(|(i, l)| render::display_line(l, hl.line(i))).collect();
        let f = Fly { pane, rows, from, to, color, at: Instant::now() };
        let t = f.total();
        self.fx.flies.push(f);
        self.fx.want_frame = true;
        t
    }

    /// Merges a floating snippet into a file: a gap opens, the lines fly over, the card fades where it stood.
    pub(crate) fn merge_fx(&mut self, s: u64, f: u64, at: usize, del: usize) -> bool {
        self.doc.sync_refs();
        let (Some(sc), Some(fc)) = (self.doc.card(s).cloned(), self.doc.card(f).cloned()) else { return false };
        if !self.doc.merge(s, f, at, del) { return false; }
        let (at, n) = (at.min(fc.lines.len()), sc.lines.len());
        let from = self.code_origin(&sc, 0);
        let to = (fc.x + self.gutter(fc.lines.len() + n - del.min(fc.lines.len())), fc.y + HEAD + PAD + at as f32 * LH);
        let hide = if self.shared || sc.pane == fc.pane { self.fly(fc.pane, &sc.lines, &fc.lang, from, to, sc.color) } else { 300. };
        self.fx.line(f, at, n, LineKind::Grow, hide);
        self.fx.line(f, at + n, del, LineKind::Shut, 0.);
        self.fx.fades.push(Fade { pane: sc.pane, x: sc.x, y: sc.y, w: sc.w, h: self.card_height(&sc), color: sc.color, title: sc.title.clone(), at: Instant::now() });
        self.fx.fresh.insert(s, later(hide));
        self.selected = HashSet::from([s]);
        self.picked = None;
        true
    }

    /// Cuts a region out into a floating card at (x, y): the file closes up and the lines fly to the card.
    pub(crate) fn detach_fx(&mut self, region: u64, x: f32, y: f32, pane: u64) -> Option<u64> {
        let r = self.doc.region(region)?.clone();
        let f = self.doc.card(r.file)?.clone();
        let from = self.code_origin(&f, r.start);
        let id = self.doc.detach_region(region, x, y, pane)?;
        let n = r.end - r.start + 1;
        self.fx.line(r.file, r.start, n, LineKind::Shut, 0.);
        self.fly_to_new(id, &f, from, r.color);
        Some(id)
    }

    pub(crate) fn copy_fx(&mut self, file: u64, a: usize, b: usize, x: f32, y: f32) -> Option<u64> {
        let f = self.doc.card(file)?.clone();
        let id = self.doc.copy_out(file, a, b, x, y)?;
        let color = self.doc.card(id).map_or(0xc792ea, |c| c.color);
        self.fly_to_new(id, &f, self.code_origin(&f, a), color);
        Some(id)
    }

    pub(crate) fn reference_fx(&mut self, region: u64, x: f32, y: f32, pane: u64) -> Option<u64> {
        let r = self.doc.region(region)?.clone();
        let f = self.doc.card(r.file)?.clone();
        let id = self.doc.make_reference(region, x, y, pane)?;
        self.fly_to_new(id, &f, self.code_origin(&f, r.start), r.color);
        Some(id)
    }

    /// The new card `id` stays hidden until lines flown from `from` in file `f` reach it, then fades in.
    fn fly_to_new(&mut self, id: u64, f: &Card, from: (f32, f32), color: u32) {
        let Some(c) = self.doc.card(id).cloned() else { return };
        let wait = if self.shared || c.pane == f.pane { self.fly(c.pane, &c.lines, &f.lang, from, self.code_origin(&c, 0), color) } else { 0. };
        self.fx.born.insert(id, later(wait));
    }

    pub(crate) fn apply_to_target(&mut self, id: u64) {
        let Some(t) = self.doc.card(id).and_then(|c| c.target.clone()) else { return };
        if self.doc.file_by_path(&t.file).is_none() { self.open(t.file.clone(), None); }
        let Some(f) = self.doc.file_by_path(&t.file).map(|c| (c.id, c.lines.len())) else { self.toast(format!("{} isn't in the project", t.file), true); return };
        self.snapshot();
        match t.replace {
            Some((a, b)) => { let a = (a - 1).min(f.1 - 1); let b = (b - 1).clamp(a, f.1 - 1); self.merge_fx(id, f.0, a, b - a + 1); }
            None => { self.merge_fx(id, f.0, t.insert_at.map_or(f.1, |n| (n - 1).min(f.1)), 0); }
        }
        self.touch();
    }

    pub(crate) fn merge_selection(&mut self, file: u64) {
        let Some(path) = self.doc.card(file).and_then(|c| c.path.clone()) else { return };
        let snips: Vec<u64> = self.selected.iter().copied().filter(|i| self.doc.card(*i).is_some_and(|c| !c.is_file())).collect();
        let (mut placed, mut unplaced) = (Vec::new(), Vec::new());
        for s in snips {
            match self.doc.card(s).and_then(|c| c.target.clone()).filter(|t| t.file == path) {
                Some(t) => placed.push((s, t.replace.map(|r| r.0).or(t.insert_at).unwrap_or(usize::MAX))),
                None => unplaced.push(s),
            }
        }
        placed.sort_by_key(|p| std::cmp::Reverse(p.1));
        for (s, _) in placed { self.apply_to_target(s); }
        if !unplaced.is_empty() { self.start_pick(unplaced, Some(file)); }
    }

    // ---------- editing ----------
    pub(crate) fn enter_edit(&mut self, id: u64, line: usize, col: usize) {
        self.exit_edit();
        let (id, line, col) = match self.doc.card(id).and_then(|c| c.ref_of).and_then(|r| self.doc.region(r).cloned()) { Some(r) => (r.file, r.start, 0), None => (id, line, col) };
        let Some(c) = self.doc.card(id) else { return };
        let mut b = TextBuffer::new(c.lines.to_vec());
        b.set_cursor(line, col, false);
        self.inputs.insert(Input::Editor(id), b);
        self.editing = Some(id);
        self.focus_input = Some(Input::Editor(id));
    }

    pub(crate) fn exit_edit(&mut self) {
        let Some(id) = self.editing.take() else { return };
        if self.focus_input == Some(Input::Editor(id)) { self.focus_input = None; }
        let Some(b) = self.inputs.remove(&Input::Editor(id)) else { return };
        let Some(c) = self.doc.card(id) else { return };
        if *c.lines == b.lines { return; }
        let is_file = c.is_file();
        self.snapshot();
        if is_file { self.doc.set_file_lines(id, b.lines); } else { self.doc.card_mut(id).unwrap().lines = Lines::new(b.lines); }
        self.touch();
    }

    pub(crate) fn begin_rename(&mut self, id: u64) {
        let t = self.doc.card(id).map(|c| c.title.clone()).or_else(|| self.doc.region(id).map(|r| r.title.clone())).unwrap_or_default();
        let mut b = TextBuffer::default();
        b.set_text(&t);
        b.select_all();
        self.inputs.insert(Input::Rename(id), b);
        self.focus_input = Some(Input::Rename(id));
    }

    fn commit_rename(&mut self, id: u64) {
        let t = self.inputs.remove(&Input::Rename(id)).map(|b| b.text()).unwrap_or_default();
        if t.trim().is_empty() { return; }
        if let Some(c) = self.doc.card_mut(id) { c.title = t.trim().into(); } else if let Some(r) = self.doc.region_mut(id) { r.title = t.trim().into(); }
        self.touch();
    }

    pub(crate) fn on_key_up(&mut self, e: &KeyUpEvent, _w: &mut Window, cx: &mut Context<Self>) { if e.keystroke.key == "space" { self.space = false; cx.notify(); } }

    pub(crate) fn on_key(&mut self, e: &KeyDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        self.mods = e.keystroke.modifiers;
        let handled = if e.keystroke.key == "escape" && self.drag.is_some() { self.cancel_drag(); true }
            else if let Some(k) = self.focus_input.clone() { self.key_in_input(k, e, cx); true }
            else { self.key_on_canvas(e, w) };
        if handled { cx.notify(); }
    }

    fn cancel_drag(&mut self) {
        match self.drag.take() {
            Some(Drag::Pan { pane, ox, oy, .. }) => { let p = self.views.get_mut(&pane).unwrap(); p.v.x = ox; p.v.y = oy; p.t = p.v; }
            Some(Drag::Move { items, .. }) => for it in items { if let Some(c) = self.doc.card_mut(it.id) { c.x = it.ox; c.y = it.oy; c.pane = it.pane; } },
            Some(Drag::Edge { id, orig, moved, .. }) => if moved { if let Some(r) = self.doc.region_mut(id) { (r.start, r.end) = orig; } self.undo.pop(); self.doomed.clear(); },
            Some(Drag::Select { id }) => if let Some(c) = self.doc.card_mut(id) { c.sel = None; },
            Some(Drag::ShiftLines { id, a, b, .. }) => if let Some(c) = self.doc.card_mut(id) { c.sel = Some((a, b)); },
            Some(Drag::Marquee { base, .. }) => self.selected = base,
            Some(Drag::Resize { id, ow, .. }) => if let Some(c) = self.doc.card_mut(id) { c.w = ow; },
            _ => {}
        }
        self.live = None;
        self.anim.clear();
    }

    fn key_in_input(&mut self, k: Input, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let ks = &e.keystroke;
        let m = ks.modifiers;
        let ctrl = m.control || m.platform;
        let key = ks.key.as_str();
        let multiline = matches!(k, Input::Editor(_) | Input::DraftEdit(_));
        match k {
            Input::Find(id) | Input::Replace(id) => if self.find_key(id, matches!(k, Input::Replace(_)), key, m) { return; },
            Input::Editor(id) if ctrl && (key == "f" || key == "h") => { self.open_find(id, key == "h"); return; }
            Input::Editor(id) if key == "f3" => { self.find_step(id, if m.shift { -1 } else { 1 }); return; }
            _ => {}
        }
        match key {
            "escape" => {
                match k {
                    Input::Editor(_) => self.exit_edit(),
                    Input::Rename(id) => { self.inputs.remove(&Input::Rename(id)); self.focus_input = None; }
                    Input::DraftEdit(_) => self.commit_draft_edit(),
                    _ => self.focus_input = None,
                }
                return;
            }
            "s" if ctrl => {
                if let Input::Editor(id) = k { self.exit_edit(); self.save_file(id); }
                return;
            }
            "enter" if !multiline && !m.shift => {
                match k {
                    Input::Chat(o) => self.submit_chat(o),
                    Input::Ask(o) => self.submit_ask(o),
                    Input::Rename(id) => { self.commit_rename(id); self.focus_input = None; }
                    _ => {}
                }
                return;
            }
            _ => {}
        }
        let is_settings = matches!(k, Input::Setting(_));
        if is_settings && key == "tab" {
            if let Input::Setting(i) = k { self.focus_input = Some(Input::Setting((i + 1) % panels::SETTING_FIELDS.len())); }
            return;
        }
        let b = self.inputs.entry(k).or_default();
        let (s, c) = (m.shift, ctrl);
        match key {
            "backspace" => b.backspace(),
            "delete" => b.delete(),
            "enter" => b.insert("\n"),
            "tab" => b.insert("    "),
            "left" => b.left(s, c),
            "right" => b.right(s, c),
            "up" => b.up(s),
            "down" => b.down(s),
            "home" => b.home(s, c),
            "end" => b.end(s, c),
            "a" if c => b.select_all(),
            "z" if c && s => b.redo(),
            "z" if c => b.undo(),
            "y" if c => b.redo(),
            "c" if c => if let Some(t) = b.selected_text() { cx.write_to_clipboard(ClipboardItem::new_string(t)); },
            "x" if c => if let Some(t) = b.selected_text() { cx.write_to_clipboard(ClipboardItem::new_string(t)); b.backspace(); },
            "v" if c => if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) { b.insert(&t); },
            _ => if !c && !m.alt { if let Some(t) = &ks.key_char { b.insert(t); } },
        }
    }

    fn key_on_canvas(&mut self, e: &KeyDownEvent, _w: &mut Window) -> bool {
        let m = e.keystroke.modifiers;
        let ctrl = m.control || m.platform;
        let pane = self.active;
        match e.keystroke.key.as_str() {
            "space" => self.space = true,
            "escape" => { self.pick = None; self.menu = None; self.show_settings = false; self.clear_line_sel(); self.selected.clear(); self.picked = None; }
            "s" if ctrl => self.save_all(),
            "z" if ctrl && m.shift => self.do_redo(),
            "z" if ctrl => self.do_undo(),
            "y" if ctrl => self.do_redo(),
            "a" if ctrl => self.selected = self.doc.cards.iter().filter(|c| self.in_pane(c, pane)).map(|c| c.id).collect(),
            "0" if ctrl => self.zoom_keys(0.),
            "=" | "+" if ctrl => self.zoom_keys(1.25),
            "-" if ctrl => self.zoom_keys(0.8),
            "\\" if ctrl => self.split(!m.shift),
            "f" | "h" if ctrl => self.open_find_here(e.keystroke.key == "h"),
            "f" if !ctrl => self.fit(pane),
            "delete" | "backspace" if !self.selected.is_empty() => {
                self.snapshot();
                let ids: Vec<u64> = self.selected.drain().collect();
                for id in ids { if self.doc.region(id).is_some() { self.doc.dissolve(&[id]); } else { self.doc.remove_card(id); } }
                self.touch();
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn do_undo(&mut self) { if let Some(d) = self.undo.pop() { self.redo.push(std::mem::replace(&mut self.doc, d)); self.touch(); } }
    pub(crate) fn do_redo(&mut self) { if let Some(d) = self.redo.pop() { self.undo.push(std::mem::replace(&mut self.doc, d)); self.touch(); } }

    // ---------- chats ----------
    pub(crate) fn toggle_chat(&mut self, id: u64) { if let Some(t) = self.doc.talk_mut(id) { t.open = !t.open; } self.touch(); }

    pub(crate) fn toggle_draft(&mut self, id: u64) {
        let owner = self.doc.draft_owner(id);
        if self.doc.talk(owner).is_some_and(|t| t.draft.is_none()) {
            let base = self.doc.base_lines(owner);
            self.doc.talk_mut(owner).unwrap().draft = Some(base);
        }
        if let Some(t) = self.doc.talk_mut(id) { t.draft_open = !t.draft_open; }
        self.touch();
    }

    pub(crate) fn begin_draft_edit(&mut self, id: u64) {
        let owner = self.doc.draft_owner(id);
        let lines = self.doc.talk(owner).and_then(|t| t.draft.clone()).unwrap_or_default();
        self.inputs.insert(Input::DraftEdit(owner), TextBuffer::new(lines));
        self.focus_input = Some(Input::DraftEdit(owner));
    }

    pub(crate) fn commit_draft_edit(&mut self) {
        let Some(Input::DraftEdit(o)) = self.focus_input.clone() else { return };
        self.focus_input = None;
        if let (Some(b), Some(t)) = (self.inputs.remove(&Input::DraftEdit(o)), self.doc.talk_mut(o)) { t.draft = Some(b.lines); }
        self.touch();
    }

    pub(crate) fn reset_draft(&mut self, id: u64) {
        let o = self.doc.draft_owner(id);
        let base = self.doc.base_lines(o);
        if let Some(t) = self.doc.talk_mut(o) { t.draft = Some(base); }
        self.inputs.remove(&Input::DraftEdit(o));
        self.touch();
    }

    pub(crate) fn apply_draft(&mut self, id: u64) {
        self.snapshot();
        let owner = self.doc.draft_owner(id);
        let added: Vec<usize> = match (self.doc.region(owner), self.doc.talk(owner).and_then(|t| t.draft.clone())) {
            (Some(r), Some(d)) => crate::diff::diff_lines(&self.doc.region_lines(r), &d).into_iter().filter_map(|o| match o { crate::diff::Op::Add(b) => Some(r.start + b), _ => None }).collect(),
            _ => Vec::new(),
        };
        let file = self.doc.region(owner).map(|r| r.file);
        if self.doc.apply_draft(id) {
            if let Some(f) = file { for l in added { self.fx.line(f, l, 1, LineKind::Flash, 0.); } }
            self.toast("Draft applied", false); let o = self.doc.draft_owner(id); let base = self.doc.base_lines(o); if let Some(t) = self.doc.talk_mut(o) { t.draft = Some(base); } self.touch(); }
        else { self.undo.pop(); self.toast("The draft has no changes.", false); }
    }

    fn context_for(&self, owner: u64) -> (Vec<agent::ContextItem>, Option<String>) {
        if owner == 0 { return (self.doc.ctx.iter().filter_map(|i| self.doc.ctx_item(*i)).collect(), None); }
        let mut items: Vec<_> = self.doc.ctx_item(owner).into_iter().collect();
        if self.doc.talk(owner).is_some_and(|t| t.global) { items.extend(self.doc.ctx.iter().filter(|i| **i != owner).filter_map(|i| self.doc.ctx_item(*i))); }
        let focus = if let Some(c) = self.doc.card(owner).filter(|c| c.is_file()) { format!("the file {}", c.path.as_deref().unwrap_or("")) } else {
            let (title, place) = match self.doc.region(owner) {
                Some(r) => (r.title.clone(), format!("{} lines {}-{}", self.doc.card(r.file).and_then(|f| f.path.clone()).unwrap_or_default(), r.start + 1, r.end + 1)),
                None => { let c = self.doc.card(owner); (c.map_or(String::new(), |c| c.title.clone()), "a free-floating snippet".to_string()) }
            };
            format!("the snippet \"{title}\" ({place}). If you propose changes to this snippet, reply with the COMPLETE revised snippet in exactly one ```snippet block without file/insert/replace attributes; it is shown to the user as a diff draft")
        };
        (items, Some(focus))
    }

    pub(crate) fn submit_chat(&mut self, owner: u64) {
        let text = self.inputs.get(&Input::Chat(owner)).map(|b| b.text()).unwrap_or_default();
        if text.trim().is_empty() || self.busy.contains_key(&owner) { return; }
        self.input(Input::Chat(owner)).set_text("");
        self.send_chat(owner, text.trim().to_string());
    }

    fn submit_ask(&mut self, owner: u64) {
        let text = self.inputs.get(&Input::Ask(owner)).map(|b| b.text()).unwrap_or_default();
        if text.trim().is_empty() { return; }
        if self.busy.contains_key(&owner) { self.toast("This snippet's chat is still answering.", false); return; }
        self.input(Input::Ask(owner)).set_text("");
        self.send_chat(owner, format!("Revise this snippet: {}", text.trim()));
    }

    pub(crate) fn send_chat(&mut self, owner: u64, text: String) {
        let Some(t) = self.doc.talk_mut(owner) else { return };
        t.chat.push(ChatMsg::new(Role::User, text));
        let history: Vec<Message> = t.chat.iter().filter(|m| m.role != Role::Error).map(|m| Message { role: if m.role == Role::User { "user" } else { "assistant" }, content: m.content.to_string() }).collect();
        let (context, focus) = self.context_for(owner);
        let cancel: Cancel = Arc::new(AtomicBool::new(false));
        self.busy.insert(owner, Busy { text: String::new(), status: String::new(), cancel: cancel.clone() });
        self.pending_bottom.insert(owner);
        let (project, tx) = (self.project.clone(), self.tx.clone());
        let cfg = self.settings.llm();
        let web = (!self.settings.searxng_url.trim().is_empty()).then(|| WebTools { searxng: self.settings.searxng_url.trim().into(), max_results: self.settings.max_results });
        let max_calls = self.settings.max_tool_calls;
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let req = ChatRequest { cfg, project: &project, web, history, context, focus, max_calls, cancel };
            let r = agent::run_chat(req, &mut |d| { let _ = tx.send(Ev::Delta(owner, d.to_string())); }, &mut |s| { let _ = tx2.send(Ev::Status(owner, s.to_string())); });
            let _ = tx.send(match r { Ok((t, d)) => Ev::Done(owner, t, d), Err(e) => Ev::Error(owner, e) });
        });
        self.touch();
    }

    pub(crate) fn stop_chat(&mut self, owner: u64) { if let Some(b) = self.busy.get(&owner) { b.cancel.store(true, Ordering::Relaxed); } }

    pub(crate) fn reset_chat(&mut self, owner: u64) { if let Some(t) = self.doc.talk_mut(owner) { t.reset_chat(); } self.touch(); }

    pub(crate) fn request_models(&mut self) {
        self.models = Some(Vec::new());
        let (cfg, tx) = (self.settings.llm(), self.tx.clone());
        std::thread::spawn(move || { let _ = tx.send(Ev::Models(list_models(&cfg))); });
    }

/// A fingerprint of everything the global chat panel shows. When it changes, the panel is redrawn;
    /// otherwise GPUI reuses its previous frame.
    pub(crate) fn chat_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let g = &self.doc.global;
        (g.chat.len(), g.chat.last().map(|m| Arc::as_ptr(&m.content) as *const u8 as usize)).hash(&mut h);
        self.busy.get(&0).map(|b| (b.text.len(), b.status.clone())).hash(&mut h);
        // an empty input box and none at all look the same (housekeeping drops empty ones)
        self.inputs.get(&Input::Chat(0)).filter(|b| !b.is_empty() || self.focus_input == Some(Input::Chat(0))).map(|b| (b.lines.clone(), b.cur.line, b.cur.col, b.anchor.map(|a| (a.line, a.col)))).hash(&mut h);
        (self.focus_input == Some(Input::Chat(0)), self.doc.ctx.len(), self.pending_bottom.contains(&0), self.win_size.1.to_bits()).hash(&mut h);
        h.finish()
    }

/// A fingerprint of everything a chrome view shows. When it changes, that view is redrawn.
    pub(crate) fn chrome_key(&self, part: Part) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.win_size.0.to_bits().hash(&mut h);
        self.win_size.1.to_bits().hash(&mut h);
        match part {
            Part::Top => {
                ((self.pv(self.active).v.z * 100.).round() as i64, self.any_dirty(), self.explain_queue.is_some(), self.shared, self.chat_open, self.show_help, self.explorer_open, self.models.is_some()).hash(&mut h);
                (&self.settings.model, &self.settings.provider, &self.settings.endpoint).hash(&mut h);
            }
            Part::Explorer => {
                self.tree.len().hash(&mut h);
                (self.tree.first(), self.tree.last()).hash(&mut h);
                let mut ex: Vec<&String> = self.expanded.iter().collect();
                ex.sort();
                ex.hash(&mut h);
                self.inputs.get(&Input::Filter).map(|b| (b.lines.clone(), b.cur.line, b.cur.col, b.anchor.map(|a| (a.line, a.col)))).hash(&mut h);
                (self.focus_input == Some(Input::Filter)).hash(&mut h);
                // which files are on the canvas (their rows are highlighted)
                self.doc.cards.iter().filter_map(|c| c.path.as_ref()).for_each(|p| p.hash(&mut h));
            }
            Part::Context => {
                (self.hist_open, self.doc.history.len()).hash(&mut h);
                for id in &self.doc.ctx { (id, self.doc.ctx_label(*id), self.doc.ctx_lines(*id)).hash(&mut h); }
            }
        }
        h.finish()
    }

    /// Whether anything is under way that the timer has to check on often.
    fn working(&self) -> bool { !self.busy.is_empty() || !self.explaining.is_empty() || self.tile_pool.in_flight > 0 || !self.tile_inbox.is_empty() || self.state_dirty.is_some() }

    /// Runs on a short timer: drains LLM events and does periodic chores.
    fn pump(&mut self) -> bool {
        let mut changed = false;
        while let Ok(path) = self.inbox.try_recv() {
            changed = true;
            if !path.is_empty() { self.open_abs(std::path::Path::new(&path)); }
        }
        while let Ok(ev) = self.rx.try_recv() { changed = true; self.handle_ev(ev); }
        if self.last_poll.elapsed() > Duration::from_millis(800) {
            self.last_poll = Instant::now();
            let before: Vec<Lines> = self.doc.cards.iter().map(|c| c.lines.clone()).collect();
            self.reload_from_disk();
            self.doc.sync_refs();
            changed |= before.len() != self.doc.cards.len() || before.iter().zip(&self.doc.cards).any(|(a, c)| !Arc::ptr_eq(a, &c.lines));
            changed |= !self.toasts.is_empty(); // they fade and expire
            self.toasts.retain(|t| t.2.elapsed() < Duration::from_secs(5));
            self.forget_closed();
        }
        if self.state_dirty.is_some_and(|t| t.elapsed() > Duration::from_millis(700)) { self.persist(); }
        changed
    }

    /// Drops caches that belong to cards and snippets which no longer exist.
    fn forget_closed(&mut self) {
        let alive: HashSet<u64> = self.doc.cards.iter().map(|c| c.id).chain(self.doc.regions.iter().map(|r| r.id)).chain([0, u64::MAX]).collect();
        // highlighting of files that haven't been drawn for a while is redone when they come back
        self.spans.retain(|id, e| alive.contains(id) && e.2.elapsed() < Duration::from_secs(30));
        self.scrolls.retain(|id, _| alive.contains(id) || alive.contains(&id.wrapping_sub(1 << 40)) || *id == u64::MAX - 1);
        self.mtimes.retain(|id, _| alive.contains(id));
        self.fx.forget_except(&alive);
        self.finds.retain(|id, _| alive.contains(id));
        self.chat_lists.retain(|id, _| alive.contains(id));
        self.retire_tiles(|k, _| !alive.contains(&k.1));
        self.inputs.retain(|k, b| match k { Input::Chat(id) | Input::Ask(id) => alive.contains(id) && !b.is_empty(), _ => true });
    }

    /// Drops the cells `gone` picks; their images are released at the next frame.
    pub(crate) fn retire_tiles(&mut self, gone: impl Fn(&tiles::Key, &tiles::Cell) -> bool) {
        let dead = &mut self.dead_tiles;
        self.tiles.retain(|k, c| { let drop = gone(k, c); if drop { dead.extend(c.img.take()); } !drop });
    }

    /// The dot-grid tile for a dot spacing (device pixels), made once and kept for the last few spacings.
    pub(crate) fn dot_tile(&mut self, gap: u32) -> Option<(Arc<gpui::RenderImage>, u32)> {
        let per = (512 / gap.max(1)).clamp(1, 16);
        if let Some(i) = self.dot_tiles.iter().position(|(g, _)| *g == gap) {
            let t = self.dot_tiles.remove(i);
            self.dot_tiles.push(t);
        } else {
            let img = tiles::dot_tile(gap, per, render::FG, 0.13)?;
            self.dot_tiles.push((gap, img));
            if self.dot_tiles.len() > 6 { let (_, old) = self.dot_tiles.remove(0); self.dead_tiles.push(old); }
        }
        self.dot_tiles.last().map(|(_, img)| (img.clone(), per))
    }

    /// Once per frame: take in the cells the workers finished (a few per frame, see `UPLOAD_PER_FRAME`),
    /// forget cells nothing has drawn for half a second (they are quick to redo), and keep the rest
    /// within a memory budget.
    pub(crate) fn tend_tiles(&mut self) {
        // About four screens of cells: what is in view (cards round up to whole cells), plus the zoom
        // being moved toward. GPUI keeps a copy of each in memory as well as on the GPU.
        let budget = ((self.win_size.0 * self.win_size.1 * self.scale * self.scale * 4. * 4.) as usize).max(16 << 20);
        let finished = self.tile_pool.finished();
        self.tile_inbox.extend(finished);
        // Cells nobody has asked for in the last few frames (the zoom moved on while they were being
        // drawn) are dropped unseen, so the ones wanted now are not kept waiting behind them.
        let frame = self.frame;
        let tiles = &mut self.tiles;
        self.tile_inbox.retain(|d| {
            let stale = tiles.get(&d.key).is_some_and(|c| c.asked && frame.saturating_sub(c.used) > 4);
            if stale { tiles.remove(&d.key); }
            !stale
        });
        let mut taken = 0;
        while taken < tiles::UPLOAD_PER_FRAME {
            let Some(d) = self.tile_inbox.pop_front() else { break };
            let Some(c) = self.tiles.get_mut(&d.key) else { continue };
            // drawn from text that has since changed: thrown away
            if !(Arc::ptr_eq(&c.lines, &d.lines) && c.start_no == d.start_no && c.scale == d.scale) { continue; }
            c.asked = false;
            match d.img { Some(img) => { c.img = Some(img); taken += 1; } None => c.missing = true }
        }
        if !self.tile_inbox.is_empty() || self.tile_pool.in_flight > 0 { self.fx.want_frame = true; }
        self.retire_tiles(|_, c| frame.saturating_sub(c.used) > 20 && !c.asked);
        // Over budget: the least recently used cells go first, never one drawn in the last two frames
        // (dropping cells still on screen would put their text back to direct drawing and redo them).
        let mut total: usize = self.tiles.values().map(tiles::Cell::bytes).sum();
        if total > budget {
            let mut old: Vec<(u64, tiles::Key)> = self.tiles.iter().filter(|(_, c)| c.img.is_some() && !c.asked && frame.saturating_sub(c.used) > 2).map(|(k, c)| (c.used, *k)).collect();
            old.sort_unstable_by_key(|o| o.0);
            let mut gone = HashSet::new();
            for (_, k) in old { if total <= budget { break; } total -= tiles::CELL_BYTES; gone.insert(k); }
            if !gone.is_empty() { self.retire_tiles(|k, _| gone.contains(k)); }
        }
        self.tile_levels.clear();
        for (k, c) in &self.tiles {
            if c.img.is_none() { continue; }
            let v = self.tile_levels.entry((k.0, k.1)).or_default();
            if !v.iter().any(|l| l.to_bits() == k.2) { v.push(f32::from_bits(k.2)); }
        }
        let cards: HashSet<(u64, u64)> = self.tiles.keys().map(|k| (k.0, k.1)).collect();
        self.tile_shown.retain(|k, _| cards.contains(k));
    }

    fn handle_ev(&mut self, ev: Ev) {
        match ev {
            Ev::Delta(o, d) => if let Some(b) = self.busy.get_mut(&o) { b.text.push_str(&d); b.status.clear(); self.pending_bottom.insert(o); },
            Ev::Status(o, s) => if let Some(b) = self.busy.get_mut(&o) { b.status = s; },
            Ev::Done(o, text, drafts) => {
                self.busy.remove(&o);
                if let Some(t) = self.doc.talk_mut(o) { t.chat.push(ChatMsg::new(Role::Assistant, text)); }
                self.pending_bottom.insert(o);
                self.route_drafts(o, drafts);
                self.touch();
            }
            Ev::Error(o, e) => {
                let partial = self.busy.remove(&o).map(|b| b.text).unwrap_or_default();
                if let Some(t) = self.doc.talk_mut(o) {
                    if !partial.is_empty() { t.chat.push(ChatMsg::new(Role::Assistant, partial)); }
                    t.chat.push(ChatMsg::new(Role::Error, e));
                }
                self.pending_bottom.insert(o);
                self.touch();
            }
            Ev::Progress(path, segs) => self.apply_segments(&path, &segs),
            Ev::ExplainDone(path, r) => {
                self.explaining.remove(&path);
                if let Err(e) = r { if e != "Stopped." { self.toast(format!("Explain {path}: {e}"), true); } else { self.explain_queue = None; } }
                self.next_explain();
                self.touch();
            }
            Ev::Models(m) => if self.models.is_some() { self.models = Some(m); },
            Ev::SaveFailed(e) => self.toast(format!("Couldn't save the canvas: {e}"), true),
        }
    }

    /// Replies aimed at this snippet become its draft; everything else becomes cards on the canvas.
    fn route_drafts(&mut self, owner: u64, mut drafts: Vec<Draft>) {
        let is_snippet = owner != 0 && !self.doc.card(owner).is_some_and(Card::is_file);
        if is_snippet {
            let o = self.doc.draft_owner(owner);
            let own = self.doc.region(o).and_then(|r| self.doc.card(r.file)).and_then(|f| f.path.clone());
            if let Some(i) = drafts.iter().position(|d| d.file.is_none() || (d.file == own && own.is_some() && (d.replace.is_some() || d.insert_at.is_none()))) {
                let d = drafts.remove(i);
                self.doc.talk_mut(o).unwrap().draft = Some(d.code.trim_end_matches('\n').split('\n').map(String::from).collect());
                if let Some(t) = self.doc.talk_mut(owner) { t.draft_open = true; }
            }
        }
        self.spawn_drafts(drafts, owner);
    }

    pub(crate) fn spawn_drafts(&mut self, drafts: Vec<Draft>, near: u64) {
        let n = drafts.len();
        for (k, d) in drafts.into_iter().enumerate() {
            let lines: Vec<String> = d.code.trim_end_matches('\n').split('\n').map(String::from).collect();
            let target = d.file.clone().map(|file| Target { file, insert_at: d.insert_at, replace: d.replace });
            if let Some(f) = d.file.clone().filter(|p| self.doc.file_by_path(p).is_none()) { self.open(f, None); }
            let anchor = d.file.as_deref().and_then(|p| self.doc.file_by_path(p)).cloned();
            let near_card = self.doc.region(near).and_then(|r| self.doc.card(r.file)).or(self.doc.card(near)).cloned();
            let pane = anchor.as_ref().or(near_card.as_ref()).map_or(self.active, |c| c.pane);
            let (x, mut y) = match (&anchor, &near_card) {
                (Some(f), _) => (f.x + f.w + 480., f.y + HEAD + PAD + d.replace.map(|r| r.0).or(d.insert_at).unwrap_or(1).saturating_sub(1) as f32 * LH - 40.),
                (None, Some(c)) => (c.x + c.w + 480., c.y + k as f32 * 40.),
                _ => { let r = self.rect_of(self.active); let (cx, cy) = self.to_world(self.active, r.x + r.w / 2., r.y + r.h / 2.); self.find_spot(420., self.active, cx, cy) }
            };
            let h = HEAD + PAD * 2. + lines.len() as f32 * LH;
            for _ in 0..60 {
                let hit = self.doc.cards.iter().filter(|c| self.in_pane(c, pane)).find(|c| x < c.x + c.w && x + 420. > c.x && y < c.y + self.card_height(c) && y + h > c.y);
                match hit { Some(c) => y = c.y + self.card_height(c) + 24., None => break }
            }
            let lang = d.file.clone().or(d.lang.map(|l| if l.contains('.') { l } else { format!("x.{l}") })).or(near_card.as_ref().map(|c| c.lang.clone())).unwrap_or_else(|| guess_lang(&lines));
            let id = self.doc.add_floating(d.title, lines, lang, x, y, 460., pane, target);
            self.fx.born.insert(id, Instant::now());
        }
        if n > 0 { self.toast(format!("{n} snippet(s) drafted on the canvas"), false); self.touch(); }
    }

    // ---------- explain ----------
    pub(crate) fn explain(&mut self, path: String) {
        if self.explaining.contains_key(&path) { return; }
        if self.doc.file_by_path(&path).is_none() { self.open(path.clone(), None); }
        let Some(f) = self.doc.file_by_path(&path).cloned() else { self.next_explain(); return };
        self.snapshot();
        let old: Vec<u64> = self.doc.regions_of(f.id).iter().filter(|r| r.explained).map(|r| r.id).collect();
        self.doc.dissolve(&old);
        let cancel: Cancel = Arc::new(AtomicBool::new(false));
        self.explaining.insert(path.clone(), cancel.clone());
        let (cfg, tx, text) = (self.settings.llm(), self.tx.clone(), f.lines.join("\n"));
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let p2 = path.clone();
            let r = agent::explain_file(&cfg, &path, &text, &cancel, &mut |s| { let _ = tx2.send(Ev::Progress(p2.clone(), s.to_vec())); });
            let _ = tx.send(Ev::ExplainDone(path, r.map(|_| ())));
        });
    }

    fn apply_segments(&mut self, path: &str, segs: &[Segment]) {
        let Some(f) = self.doc.file_by_path(path).map(|c| (c.id, c.lines.len())) else { return };
        let have = self.doc.regions_of(f.0).iter().filter(|r| r.explained).count();
        for g in segs.iter().skip(have) {
            let a = (g.start - 1).min(f.1 - 1);
            let b = (g.end - 1).clamp(a, f.1 - 1);
            let mut note = ChatMsg::new(Role::Note, g.explanation.clone());
            note.title = Some(g.title.clone());
            self.doc.create_region(f.0, a, b, Some(g.title.clone()), true, vec![note]);
        }
        self.touch();
    }

    pub(crate) fn explain_all(&mut self) {
        if self.explain_queue.is_some() { self.stop_explain(); return; }
        const SKIP: [&str; 19] = ["png", "jpg", "jpeg", "gif", "ico", "svg", "woff", "woff2", "ttf", "lock", "map", "pdf", "zip", "exe", "dll", "bin", "wasm", "so", "dylib"];
        let files: Vec<String> = self.tree.iter().filter(|f| !f.rsplit_once('.').is_some_and(|(_, e)| SKIP.contains(&e.to_lowercase().as_str()))).filter(|f| !matches!(basename(f), "package-lock.json" | "yarn.lock" | "Cargo.lock")).take(self.settings.explain_all_max_files).cloned().collect();
        if files.is_empty() { self.toast("No files to explain.", false); return; }
        for p in &files { if self.doc.file_by_path(p).is_none() { self.open(p.clone(), Some((0., 0., self.active))); } }
        self.arrange(self.active);
        self.explain_queue = Some((files, 0));
        self.next_explain();
    }

    fn next_explain(&mut self) {
        let Some((files, i)) = &mut self.explain_queue else { return };
        if *i >= files.len() { let n = files.len(); self.explain_queue = None; self.toast(format!("Explained {n} file(s)"), false); return; }
        let p = files[*i].clone();
        *i += 1;
        self.explain(p);
    }

    pub(crate) fn stop_explain(&mut self) { self.explain_queue = None; for c in self.explaining.values() { c.store(true, Ordering::Relaxed); } }

    pub(crate) fn arrange(&mut self, pane: u64) {
        let mut files: Vec<(String, u64, f32, f32)> = self.doc.cards.iter().filter(|c| c.is_file() && self.in_pane(c, pane)).map(|c| (c.path.clone().unwrap(), c.id, c.w, self.card_height(c))).collect();
        if files.is_empty() { return; }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let cols = ((files.len() as f32 / 1.5).sqrt().ceil() as usize).clamp(1, 6);
        let col_w = files.iter().map(|f| f.2).fold(0., f32::max) + 320.;
        let mut heights = vec![0f32; cols];
        for (_, id, _, h) in &files {
            let i = heights.iter().enumerate().min_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
            let c = self.doc.card_mut(*id).unwrap();
            let old = (c.x, c.y);
            c.x = i as f32 * col_w; c.y = heights[i]; c.pane = pane;
            self.fx.slides.insert(*id, (old.0 - c.x, old.1 - c.y, Instant::now()));
            heights[i] += h + 60.;
        }
        let h = heights.iter().cloned().fold(0., f32::max).min(4000.);
        self.frame(pane, 0., 0., cols as f32 * col_w, h, 0.9);
        self.touch();
    }

    pub(crate) fn dissolve_all(&mut self, file: Option<u64>) {
        self.snapshot();
        let n = self.doc.dissolve_all(file);
        if n == 0 { self.undo.pop(); self.toast("No snippets to dissolve.", false); } else { self.toast(format!("Dissolved {n} snippet(s); the code stays. Ctrl+Z undoes."), false); self.touch(); }
    }

    pub(crate) fn toggle_ctx(&mut self, id: u64) { self.doc.toggle_ctx(id, now_secs()); self.touch(); }

    pub(crate) fn toggle_notes(&mut self, file: u64) {
        let regs = self.doc.regions_of(file);
        let open = !regs.iter().any(|r| r.talk.open);
        for r in regs { self.doc.region_mut(r.id).unwrap().talk.open = open; }
        self.touch();
    }

    pub(crate) fn make_selection_region(&mut self, file: u64) -> Option<u64> {
        let (a, b) = self.doc.card(file)?.sel_range()?;
        self.snapshot();
        let r = self.doc.create_region(file, a, b, None, false, vec![]);
        self.doc.card_mut(file)?.sel = None;
        self.touch();
        Some(r)
    }

    pub(crate) fn set_spans(&mut self, id: u64, lines: &[String], lang: &str) {
        let print = fingerprint(lines);
        if let Some(e) = self.spans.get_mut(&id).filter(|(p, ..)| matches!(p, SpanKey::Print(x) if *x == print)) { e.2 = Instant::now(); return; }
        self.spans.insert(id, (SpanKey::Print(print), Rc::new(highlight(lines, lang)), Instant::now()));
    }

    /// The same for a card's own text. The cache holds on to the lines it highlighted, and shared lines
    /// are copied before they are written, so "the same allocation" means "the same text": checking
    /// costs nothing, however large the file.
    pub(crate) fn set_spans_shared(&mut self, id: u64, lines: &Lines, lang: &str) {
        if let Some(e) = self.spans.get_mut(&id).filter(|(p, ..)| matches!(p, SpanKey::Shared(l) if Arc::ptr_eq(l, lines))) { e.2 = Instant::now(); return; }
        self.spans.insert(id, (SpanKey::Shared(lines.clone()), Rc::new(highlight(lines, lang)), Instant::now()));
    }
}

/// A quick 64-bit fingerprint of some lines (eight bytes at a time), to notice when text changed
/// without keeping a second copy of it around to compare against.
pub(crate) fn fingerprint(lines: &[String]) -> u64 {
    const K: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut h = (lines.len() as u64).wrapping_mul(K);
    for l in lines {
        h = (h ^ l.len() as u64).wrapping_mul(K);
        let mut chunks = l.as_bytes().chunks_exact(8);
        for c in &mut chunks { h = (h.rotate_left(27) ^ u64::from_le_bytes(c.try_into().unwrap())).wrapping_mul(K); }
        for b in chunks.remainder() { h = (h.rotate_left(9) ^ *b as u64).wrapping_mul(K); }
    }
    h
}

pub(crate) fn visual_len(s: &str) -> usize { s.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum() }
fn visual_to_chars(s: &str, vis: usize) -> usize {
    let (mut v, mut n) = (0, 0);
    for c in s.chars() {
        if v >= vis { break; }
        v += if c == '\t' { 4 } else { 1 };
        n += 1;
    }
    n
}
