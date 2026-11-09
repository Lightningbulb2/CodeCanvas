//! Editor-independent state: project access, XDG paths, cards, snippets, references, drafts, persistence.

use crate::agent::ContextItem;
use crate::diff::{Op, diff_lines};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_FILES: usize = 8000;
const COLORS: [u32; 8] = [0xc792ea, 0x82aaff, 0xf78c6c, 0x7fdbca, 0xffcb6b, 0xff5370, 0xc3e88d, 0x89ddff];

/// XDG Base Directory locations. Unset, empty or relative values fall back to the spec's defaults.
pub fn xdg_dir(env: &HashMap<String, String>, home: &Path, var: &str, fallback: &[&str]) -> PathBuf {
    let base = env.get(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| {
        fallback.iter().fold(home.to_path_buf(), |p, s| p.join(s))
    });
    base.join("codecanvas")
}

fn platform_dir(var: &str, fallback: &[&str]) -> PathBuf {
    let env: HashMap<String, String> = std::env::vars().collect();
    if cfg!(windows) {
        return PathBuf::from(env.get("APPDATA").cloned().unwrap_or_else(|| ".".into())).join("CodeCanvas-rust");
    }
    let home = PathBuf::from(env.get("HOME").cloned().unwrap_or_else(|| ".".into()));
    xdg_dir(&env, &home, var, fallback)
}

/// `$XDG_STATE_HOME/codecanvas`: per-project canvas state.
pub fn state_dir() -> PathBuf { platform_dir("XDG_STATE_HOME", &[".local", "state"]) }
/// `$XDG_CONFIG_HOME/codecanvas`: settings.
pub fn config_dir() -> PathBuf { platform_dir("XDG_CONFIG_HOME", &[".config"]) }
/// `$XDG_RUNTIME_DIR/codecanvas`, or the state dir when no runtime dir exists.
pub fn runtime_dir() -> PathBuf {
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(d) if !cfg!(windows) && Path::new(&d).is_absolute() => PathBuf::from(d).join("codecanvas"),
        _ => state_dir().join("run"),
    }
}

pub fn hash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a
    for b in s.bytes() { h = (h ^ b as u64).wrapping_mul(0x100000001b3); }
    h
}

pub struct Project {
    pub root: PathBuf,
    real_root: PathBuf,
    pub excluded: Vec<String>,
}

impl Project {
    pub fn new(root: PathBuf, excluded: Vec<String>) -> std::io::Result<Self> {
        let real_root = fs::canonicalize(&root)?;
        Ok(Self { root, real_root, excluded })
    }

    pub fn id(&self) -> u64 { hash_str(&self.real_root.to_string_lossy()) }

    /// Absolute path of a project-relative path, or `None` if it leads outside the project.
    pub fn resolve(&self, rel: &str, must_exist: bool) -> Option<PathBuf> {
        if Path::new(rel).is_absolute() || rel.split(['/', '\\']).any(|s| s == "..") { return None; }
        let abs = self.root.join(rel);
        match fs::canonicalize(&abs) {
            Ok(real) => real.starts_with(&self.real_root).then_some(abs),
            Err(_) if !must_exist => Some(abs),
            Err(_) => None,
        }
    }

    pub fn tree(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut pending = vec![String::new()];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = fs::read_dir(self.root.join(&dir)) else { continue };
            for e in entries.flatten() {
                if out.len() >= MAX_FILES { break; }
                let name = e.file_name().to_string_lossy().into_owned();
                let rel = if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") };
                match e.file_type() {
                    Ok(t) if t.is_dir() => if name != ".git" && !self.excluded.iter().any(|x| *x == name) { pending.push(rel); },
                    Ok(t) if t.is_file() => out.push(rel),
                    _ => {}
                }
            }
        }
        out.sort();
        out
    }

    /// File text, or `None` if missing, outside the project, or binary.
    pub fn read(&self, rel: &str) -> Option<String> {
        let bytes = fs::read(self.resolve(rel, true)?).ok()?;
        if bytes.iter().take(8000).any(|b| *b == 0) { return None; }
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn write(&self, rel: &str, text: &str) -> Result<(), String> {
        let abs = self.resolve(rel, false).ok_or_else(|| format!("{rel} is outside the project"))?;
        fs::write(abs, text).map_err(|e| e.to_string())
    }

    fn state_file(&self) -> PathBuf { state_dir().join("projects").join(format!("{:016x}.json", self.id())) }

    pub fn save_state(&self, s: &Saved) -> std::io::Result<()> {
        let file = self.state_file();
        fs::create_dir_all(file.parent().unwrap())?;
        let tmp = file.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec(s)?)?;
        fs::rename(tmp, file)
    }

    pub fn load_state(&self) -> Option<Saved> { serde_json::from_slice(&fs::read(self.state_file()).ok()?).ok() }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum Role { User, Assistant, Note, Error }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChatMsg {
    pub role: Role,
    pub content: String,
    #[serde(default)]
    pub title: Option<String>,
}

impl ChatMsg {
    pub fn new(role: Role, content: impl Into<String>) -> Self { Self { role, content: content.into(), title: None } }
}

/// A conversation plus a draft: every snippet, file and the global chat has one.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Talk {
    pub chat: Vec<ChatMsg>,
    pub global: bool,
    pub open: bool,
    pub draft: Option<Vec<String>>,
    pub draft_open: bool,
}

impl Talk {
    pub fn reset_chat(&mut self) { self.chat.retain(|m| m.role == Role::Note); }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Target {
    pub file: String,
    pub insert_at: Option<usize>,      // 1-based
    pub replace: Option<(usize, usize)>, // 1-based inclusive
}

/// A file on the canvas, or a floating snippet (`path` is `None`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Card {
    pub id: u64,
    pub path: Option<String>,
    pub title: String,
    pub lang: String,
    pub pane: u64,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub lines: Vec<String>,
    #[serde(default)]
    pub disk: Vec<String>,
    pub color: u32,
    #[serde(default)]
    pub talk: Talk,
    /// A reference card mirrors this region.
    #[serde(default)]
    pub ref_of: Option<u64>,
    #[serde(default)]
    pub target: Option<Target>,
    #[serde(skip)]
    pub sel: Option<(usize, usize)>,
}

impl Card {
    pub fn is_file(&self) -> bool { self.path.is_some() }
    pub fn dirty(&self) -> bool { self.is_file() && self.lines != self.disk }
    pub fn sel_range(&self) -> Option<(usize, usize)> { self.sel.map(|(a, b)| (a.min(b), a.max(b))) }
}

/// A snippet living inside a file: a named range of its lines.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Region {
    pub id: u64,
    pub file: u64,
    pub start: usize,
    pub end: usize,
    pub title: String,
    pub color: u32,
    #[serde(default)]
    pub explained: bool,
    #[serde(default)]
    pub talk: Talk,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Hist { pub added: bool, pub text: String, pub lines: usize, pub at: u64, pub reference: u64 }

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Doc {
    pub cards: Vec<Card>,
    pub regions: Vec<Region>,
    pub ctx: Vec<u64>,
    pub history: Vec<Hist>,
    pub global: Talk,
    pub next_id: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Layout { Pane(u64), Split { horizontal: bool, children: Vec<Layout>, sizes: Vec<f32> } }

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct View { pub x: f32, pub y: f32, pub z: f32 }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Saved {
    pub doc: Doc,
    pub layout: Layout,
    pub views: Vec<(u64, View)>,
    pub shared: bool,
    pub active: u64,
}

pub fn split_lines(text: &str) -> Vec<String> { text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect() }
pub fn eol_of(text: &str) -> &'static str { if text.contains("\r\n") { "\r\n" } else { "\n" } }
pub fn basename(p: &str) -> &str { p.rsplit('/').next().unwrap_or(p) }

pub fn guess_title(lines: &[String], path: &str, start: usize) -> String {
    const KW: [&str; 11] = ["function", "def", "fn", "class", "struct", "interface", "enum", "type", "const", "let", "local"];
    for l in lines {
        let words: Vec<&str> = l.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')).filter(|w| !w.is_empty()).collect();
        for w in words.windows(2) {
            if KW.contains(&w[0]) && !KW.contains(&w[1]) { return w[1].to_string(); }
        }
    }
    format!("{}:{}-{}", basename(path), start + 1, start + lines.len())
}

/// Sniffs a language from the code, as a path-like hint for the highlighter.
pub fn guess_lang(lines: &[String]) -> String {
    let t = lines.iter().take(80).map(String::as_str).collect::<Vec<_>>().join("\n");
    let braces = t.contains('{') || t.contains(';');
    let lua = t.lines().any(|l| { let s = l.trim_start(); s.starts_with("local ") || s.starts_with("--") || s == "end" });
    if lua && !braces { return "x.lua".into(); }
    if t.lines().any(|l| { let s = l.trim_start(); (s.starts_with("def ") || s.starts_with("class ")) && s.ends_with(':') }) { return "x.py".into(); }
    if t.lines().any(|l| l.trim_start().starts_with("#include")) { return "x.cpp".into(); }
    "x.ts".into()
}

impl Doc {
    pub fn fresh_id(&mut self) -> u64 { self.next_id += 1; self.next_id }
    fn fresh_color(&self) -> u32 { COLORS[(self.next_id as usize) % COLORS.len()] }

    pub fn card(&self, id: u64) -> Option<&Card> { self.cards.iter().find(|c| c.id == id) }
    pub fn card_mut(&mut self, id: u64) -> Option<&mut Card> { self.cards.iter_mut().find(|c| c.id == id) }
    pub fn region(&self, id: u64) -> Option<&Region> { self.regions.iter().find(|r| r.id == id) }
    pub fn region_mut(&mut self, id: u64) -> Option<&mut Region> { self.regions.iter_mut().find(|r| r.id == id) }
    pub fn file_by_path(&self, p: &str) -> Option<&Card> { self.cards.iter().find(|c| c.path.as_deref() == Some(p)) }

    pub fn regions_of(&self, file: u64) -> Vec<Region> {
        let mut v: Vec<Region> = self.regions.iter().filter(|r| r.file == file).cloned().collect();
        v.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
        v
    }

    /// `0` is the global chat.
    pub fn talk(&self, id: u64) -> Option<&Talk> {
        if id == 0 { return Some(&self.global); }
        self.card(id).map(|c| &c.talk).or_else(|| self.region(id).map(|r| &r.talk))
    }
    pub fn talk_mut(&mut self, id: u64) -> Option<&mut Talk> {
        if id == 0 { return Some(&mut self.global); }
        if self.card(id).is_some() { return self.card_mut(id).map(|c| &mut c.talk); }
        self.region_mut(id).map(|r| &mut r.talk)
    }

    pub fn region_lines(&self, r: &Region) -> Vec<String> {
        self.card(r.file).map(|f| f.lines[r.start.min(f.lines.len())..(r.end + 1).min(f.lines.len())].to_vec()).unwrap_or_default()
    }

    pub fn is_referenced(&self, region: u64) -> bool { self.cards.iter().any(|c| c.ref_of == Some(region)) }

    /// Keeps reference cards mirroring their source; a card whose source is gone becomes a plain snippet.
    pub fn sync_refs(&mut self) {
        let snapshot: Vec<(usize, Option<Vec<String>>)> = self.cards.iter().enumerate().filter_map(|(i, c)| {
            c.ref_of.map(|r| (i, self.region(r).map(|r| self.region_lines(r))))
        }).collect();
        for (i, lines) in snapshot {
            match lines { Some(l) => self.cards[i].lines = l, None => self.cards[i].ref_of = None }
        }
    }

    fn drop_regions(&mut self, ids: &[u64]) {
        self.sync_refs();
        for id in ids {
            for c in self.cards.iter_mut().filter(|c| c.ref_of == Some(*id)) { c.ref_of = None; }
            self.ctx.retain(|x| x != id);
        }
        self.regions.retain(|r| !ids.contains(&r.id));
    }

    /// Line-precise splice that keeps regions attached to the right lines.
    pub fn splice(&mut self, file: u64, at: usize, del: usize, ins: Vec<String>) {
        let d = ins.len() as isize - del as isize;
        let add = |v: usize| (v as isize + d) as usize;
        let mut dead = Vec::new();
        for r in self.regions.iter_mut().filter(|r| r.file == file) {
            if del == 0 {
                if at <= r.start { r.start = add(r.start); r.end = add(r.end); }
                else if at <= r.end { r.end = add(r.end); }
                continue;
            }
            let del_end = at + del - 1;
            if r.end < at { continue; }
            if r.start > del_end { r.start = add(r.start); r.end = add(r.end); continue; }
            if at >= r.start && del_end <= r.end {
                if r.end as isize + d < r.start as isize { dead.push(r.id); } else { r.end = add(r.end); }
                continue;
            }
            let survivors: Vec<usize> = (r.start..=r.end).filter_map(|i| if i < at { Some(i) } else if i > del_end { Some(add(i)) } else { None }).collect();
            match (survivors.first(), survivors.last()) {
                (Some(a), Some(b)) => { r.start = *a; r.end = *b; }
                _ => dead.push(r.id),
            }
        }
        // copy the mirrors before their sources disappear
        self.drop_regions(&dead);
        if let Some(f) = self.card_mut(file) {
            f.lines.splice(at..at + del, ins);
            if f.lines.is_empty() { f.lines.push(String::new()); }
            f.sel = None;
        }
        self.sync_refs();
    }

    /// Replaces a file's lines through a common prefix/suffix diff, so regions survive edits.
    pub fn set_file_lines(&mut self, file: u64, next: Vec<String>) -> bool {
        let Some(old) = self.card(file).map(|f| f.lines.clone()) else { return false };
        if old == next { return false; }
        let p = old.iter().zip(&next).take_while(|(a, b)| a == b).count();
        let max = old.len().min(next.len()) - p;
        let s = old.iter().rev().zip(next.iter().rev()).take(max).take_while(|(a, b)| a == b).count();
        self.splice(file, p, old.len() - s - p, next[p..next.len() - s].to_vec());
        true
    }

    /// Reload a file from disk unless the canvas has unsaved edits.
    pub fn apply_disk_text(&mut self, file: u64, text: &str) -> bool {
        let next = split_lines(text);
        let Some(c) = self.card(file) else { return false };
        if next == c.disk { return false; }
        let was_dirty = c.dirty();
        self.card_mut(file).unwrap().disk = next.clone();
        if was_dirty { return false; }
        self.set_file_lines(file, next);
        true
    }

    pub fn create_region(&mut self, file: u64, a: usize, b: usize, title: Option<String>, explained: bool, chat: Vec<ChatMsg>) -> u64 {
        let f = self.card(file).expect("file");
        let lines = f.lines[a..=b].to_vec();
        let title = title.unwrap_or_else(|| guess_title(&lines, f.path.as_deref().unwrap_or(""), a));
        let color = self.fresh_color();
        let id = self.fresh_id();
        self.regions.push(Region { id, file, start: a, end: b, title, color, explained, talk: Talk { chat, ..Default::default() } });
        id
    }

    fn new_floating(&mut self, title: String, lines: Vec<String>, lang: String, x: f32, y: f32, w: f32, pane: u64) -> Card {
        let color = self.fresh_color();
        let id = self.fresh_id();
        Card { id, path: None, title, lang, pane, x, y, w, lines, disk: Vec::new(), color, talk: Talk::default(), ref_of: None, target: None, sel: None }
    }

    pub fn add_floating(&mut self, title: String, lines: Vec<String>, lang: String, x: f32, y: f32, w: f32, pane: u64, target: Option<Target>) -> u64 {
        let mut c = self.new_floating(title, lines, lang, x, y, w, pane);
        c.target = target;
        let id = c.id;
        self.cards.push(c);
        id
    }

    /// A card that mirrors a region; the source stays in the file.
    pub fn make_reference(&mut self, region: u64, x: f32, y: f32, pane: u64) -> Option<u64> {
        let r = self.region(region)?.clone();
        let f = self.card(r.file)?;
        let (w, lang) = (f.w, f.lang.clone());
        let lines = self.region_lines(&r);
        let mut c = self.new_floating(r.title.clone(), lines, lang, x, y, w, pane);
        c.color = r.color;
        c.ref_of = Some(region);
        let id = c.id;
        self.cards.push(c);
        Some(id)
    }

    /// Cuts a region out of its file into a floating card; its chat and draft go with it.
    pub fn detach_region(&mut self, region: u64, x: f32, y: f32, pane: u64) -> Option<u64> {
        let r = self.region(region)?.clone();
        let f = self.card(r.file)?;
        let (w, lang) = (f.w, f.lang.clone());
        let lines = self.region_lines(&r);
        let n = lines.len();
        self.regions.retain(|x| x.id != region);
        self.ctx.retain(|x| *x != region);
        for c in self.cards.iter_mut().filter(|c| c.ref_of == Some(region)) { c.ref_of = None; }
        self.splice(r.file, r.start, n, Vec::new());
        let mut c = self.new_floating(r.title, lines, lang, x, y, w, pane);
        c.id = region; // keeps identity, so the context list and tabs still refer to it
        c.color = r.color;
        c.talk = r.talk;
        self.cards.push(c);
        Some(region)
    }

    pub fn copy_out(&mut self, file: u64, a: usize, b: usize, x: f32, y: f32) -> Option<u64> {
        let f = self.card(file)?;
        let lines = f.lines[a..=b].to_vec();
        let (w, lang, pane) = (f.w, f.lang.clone(), f.pane);
        let title = guess_title(&lines, f.path.as_deref().unwrap_or(""), a);
        Some(self.add_floating(title, lines, lang, x, y, w, pane, None))
    }

    /// Inserts a floating snippet before line `at`, replacing `del` lines; it lives on as a region of the file.
    pub fn merge(&mut self, snippet: u64, file: u64, at: usize, del: usize) -> bool {
        self.sync_refs();
        let Some(s) = self.cards.iter().position(|c| c.id == snippet && !c.is_file()) else { return false };
        if !self.card(file).is_some_and(Card::is_file) { return false; }
        let c = self.cards.remove(s);
        let at = at.min(self.card(file).unwrap().lines.len());
        let n = c.lines.len();
        self.splice(file, at, del, c.lines);
        self.regions.push(Region { id: c.id, file, start: at, end: at + n - 1, title: c.title, color: c.color, explained: false, talk: c.talk });
        true
    }

    /// Moves a region; `at` is in the destination's lines with the region already removed.
    pub fn move_region(&mut self, region: u64, to: u64, at: usize) -> bool {
        let Some(r) = self.region(region).cloned() else { return false };
        let lines = self.region_lines(&r);
        let n = lines.len();
        self.regions.iter_mut().find(|x| x.id == region).unwrap().file = u64::MAX; // protect it from the splices
        self.splice(r.file, r.start, n, Vec::new());
        self.splice(to, at, 0, lines);
        let m = self.region_mut(region).unwrap();
        m.file = to; m.start = at; m.end = at + n - 1;
        self.sync_refs();
        true
    }

    /// Moves plain selected lines inside a file; regions wholly inside travel along.
    pub fn move_lines(&mut self, file: u64, a: usize, b: usize, at: usize) {
        let Some(f) = self.card(file) else { return };
        let lines = f.lines[a..=b].to_vec();
        let n = lines.len();
        let riders: Vec<(u64, usize, usize)> = self.regions.iter().filter(|r| r.file == file && r.start >= a && r.end <= b).map(|r| (r.id, r.start - a, r.end - r.start)).collect();
        for (id, ..) in &riders { self.region_mut(*id).unwrap().file = u64::MAX; }
        self.splice(file, a, n, Vec::new());
        self.splice(file, at, 0, lines);
        for (id, d, len) in riders {
            let r = self.region_mut(id).unwrap();
            r.file = file; r.start = at + d; r.end = at + d + len;
        }
        self.sync_refs();
    }

    /// Regions a resized region would run into (a region fully containing it is left alone).
    pub fn resize_victims(&self, region: u64) -> Vec<u64> {
        let Some(s) = self.region(region) else { return Vec::new() };
        self.regions.iter().filter(|o| o.id != region && o.file == s.file && o.start <= s.end && o.end >= s.start && !(o.start <= s.start && o.end >= s.end)).map(|o| o.id).collect()
    }

    pub fn dissolve(&mut self, ids: &[u64]) { self.drop_regions(ids); }

    pub fn dissolve_all(&mut self, file: Option<u64>) -> usize {
        let ids: Vec<u64> = self.regions.iter().filter(|r| file.is_none_or(|f| r.file == f)).map(|r| r.id).collect();
        self.drop_regions(&ids);
        ids.len()
    }

    pub fn delete_region_and_lines(&mut self, region: u64) {
        let Some(r) = self.region(region).cloned() else { return };
        self.drop_regions(&[region]);
        self.splice(r.file, r.start, r.end - r.start + 1, Vec::new());
    }

    pub fn remove_card(&mut self, id: u64) {
        if self.card(id).is_some_and(Card::is_file) {
            let ids: Vec<u64> = self.regions.iter().filter(|r| r.file == id).map(|r| r.id).collect();
            self.drop_regions(&ids);
        }
        self.cards.retain(|c| c.id != id);
        self.ctx.retain(|x| *x != id);
    }

    pub fn combine(&mut self, ids: &[u64]) -> Option<u64> {
        let (first, rest) = ids.split_first()?;
        for id in rest {
            let c = self.card(*id)?.clone();
            let f = self.card_mut(*first)?;
            f.lines.extend(c.lines);
            f.talk.chat.extend(c.talk.chat);
            self.remove_card(*id);
        }
        let f = self.card_mut(*first)?;
        f.title = format!("{} +{}", f.title, rest.len());
        Some(*first)
    }

    // ---------- drafts ----------

    /// The snippet that actually owns the draft (a reference shares its source's).
    pub fn draft_owner(&self, id: u64) -> u64 { self.card(id).and_then(|c| c.ref_of).filter(|r| self.region(*r).is_some()).unwrap_or(id) }

    pub fn base_lines(&self, id: u64) -> Vec<String> {
        if let Some(r) = self.region(id) { return self.region_lines(r); }
        self.card(id).map(|c| c.lines.clone()).unwrap_or_default()
    }

    pub fn draft_stats(&self, id: u64) -> Option<(usize, usize)> {
        let owner = self.draft_owner(id);
        let draft = self.talk(owner)?.draft.as_ref()?;
        let ops = diff_lines(&self.base_lines(owner), draft);
        Some((ops.iter().filter(|o| matches!(o, Op::Add(_))).count(), ops.iter().filter(|o| matches!(o, Op::Del(_))).count()))
    }

    pub fn apply_draft(&mut self, id: u64) -> bool {
        let owner = self.draft_owner(id);
        let Some(next) = self.talk(owner).and_then(|t| t.draft.clone()) else { return false };
        if next == self.base_lines(owner) { return false; }
        if let Some(r) = self.region(owner).cloned() {
            let n = r.end - r.start + 1;
            self.talk_mut(owner).unwrap().draft = None;
            self.splice(r.file, r.start, n, next);
        } else if let Some(c) = self.card_mut(owner) {
            c.lines = next;
            c.talk.draft = None;
        }
        self.sync_refs();
        true
    }

    // ---------- context ----------

    pub fn ctx_item(&self, id: u64) -> Option<ContextItem> {
        if let Some(c) = self.card(id) {
            if let Some(p) = &c.path { return Some(ContextItem { is_file: true, label: p.clone(), path: Some(p.clone()), start_line: None, text: c.lines.join("\n") }); }
            if let Some(r) = c.ref_of.and_then(|r| self.region(r)) {
                return Some(ContextItem { is_file: false, label: c.title.clone(), path: self.card(r.file).and_then(|f| f.path.clone()), start_line: Some(r.start + 1), text: self.region_lines(r).join("\n") });
            }
            let path = c.target.as_ref().map(|t| t.file.clone());
            let start = c.target.as_ref().and_then(|t| t.replace.map(|r| r.0).or(t.insert_at));
            return Some(ContextItem { is_file: false, label: c.title.clone(), path, start_line: start, text: c.lines.join("\n") });
        }
        let r = self.region(id)?;
        Some(ContextItem { is_file: false, label: r.title.clone(), path: self.card(r.file).and_then(|f| f.path.clone()), start_line: Some(r.start + 1), text: self.region_lines(r).join("\n") })
    }

    pub fn ctx_lines(&self, id: u64) -> usize { self.ctx_item(id).map_or(0, |i| i.text.split('\n').count()) }
    pub fn ctx_label(&self, id: u64) -> String { self.ctx_item(id).map_or("?".into(), |i| i.label) }

    pub fn toggle_ctx(&mut self, id: u64, now: u64) {
        let (label, lines) = (self.ctx_label(id), self.ctx_lines(id));
        let kind = if self.card(id).is_some_and(Card::is_file) { "file" } else { "snippet" };
        let added = !self.ctx.contains(&id);
        if added { self.ctx.push(id); } else { self.ctx.retain(|x| *x != id); }
        self.history.push(Hist { added, text: format!("{kind} {label}"), lines, at: now, reference: id });
        if self.history.len() > 300 { self.history.remove(0); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    fn doc(lines: &[&str]) -> (Doc, u64) {
        let mut d = Doc::default();
        let id = d.fresh_id();
        d.cards.push(Card { id, path: Some("a.lua".into()), title: "a.lua".into(), lang: "a.lua".into(), pane: 1, x: 0., y: 0., w: 500., lines: s(lines), disk: s(lines), color: 0, talk: Talk::default(), ref_of: None, target: None, sel: None });
        (d, id)
    }

    #[test]
    fn xdg_follows_the_spec() {
        let home = Path::new("/home/u");
        let want = PathBuf::from("/home/u").join(".local").join("state").join("codecanvas");
        let mut env = HashMap::new();
        assert_eq!(xdg_dir(&env, home, "XDG_STATE_HOME", &[".local", "state"]), want);
        env.insert("XDG_STATE_HOME".to_string(), String::new());
        assert_eq!(xdg_dir(&env, home, "XDG_STATE_HOME", &[".local", "state"]), want);
        env.insert("XDG_STATE_HOME".to_string(), "relative/path".to_string());
        assert_eq!(xdg_dir(&env, home, "XDG_STATE_HOME", &[".local", "state"]), want);
    }

    #[test]
    fn regions_follow_their_lines() {
        let (mut d, f) = doc(&["a", "b", "c", "d", "e"]);
        let r = d.create_region(f, 1, 3, None, false, vec![]);
        d.splice(f, 0, 0, s(&["x"])); // before: shifts
        assert_eq!((d.region(r).unwrap().start, d.region(r).unwrap().end), (2, 4));
        d.splice(f, 3, 0, s(&["y"])); // inside: grows
        assert_eq!((d.region(r).unwrap().start, d.region(r).unwrap().end), (2, 5));
        d.splice(f, 4, 1, s(&["z", "w"])); // edit inside: resizes
        assert_eq!(d.region(r).unwrap().end, 6);
        d.splice(f, 0, 8, vec![]); // fully deleted: gone
        assert!(d.region(r).is_none());
    }

    #[test]
    fn detach_merge_keeps_identity_and_chat() {
        let (mut d, f) = doc(&["a", "b", "c", "d"]);
        let r = d.create_region(f, 1, 2, Some("mid".into()), false, vec![ChatMsg::new(Role::User, "hi")]);
        d.detach_region(r, 5., 5., 1).unwrap();
        assert_eq!(d.card(f).unwrap().lines, s(&["a", "d"]));
        let c = d.card(r).unwrap();
        assert_eq!((c.lines.clone(), c.talk.chat.len(), c.lang.as_str()), (s(&["b", "c"]), 1, "a.lua"));
        assert!(d.merge(r, f, 2, 0));
        assert_eq!(d.card(f).unwrap().lines, s(&["a", "d", "b", "c"]));
        let m = d.region(r).unwrap();
        assert_eq!((m.start, m.end, m.talk.chat.len()), (2, 3, 1));
        assert!(d.card(r).is_none());
    }

    #[test]
    fn move_region_within_and_between_files() {
        let (mut d, f) = doc(&["a", "b", "c", "d"]);
        let r = d.create_region(f, 0, 1, None, false, vec![]);
        assert!(d.move_region(r, f, 2));
        assert_eq!(d.card(f).unwrap().lines, s(&["c", "d", "a", "b"]));
        assert_eq!((d.region(r).unwrap().start, d.region(r).unwrap().end), (2, 3));
    }

    #[test]
    fn references_mirror_and_unlink() {
        let (mut d, f) = doc(&["a", "b", "c"]);
        let r = d.create_region(f, 1, 1, None, false, vec![]);
        let c = d.make_reference(r, 0., 0., 1).unwrap();
        assert!(d.is_referenced(r));
        d.splice(f, 1, 1, s(&["B"]));
        assert_eq!(d.card(c).unwrap().lines, s(&["B"]));
        d.dissolve(&[r]);
        let card = d.card(c).unwrap();
        assert_eq!((card.ref_of, card.lines.clone()), (None, s(&["B"])));
    }

    #[test]
    fn line_moves_carry_regions_inside_them() {
        let (mut d, f) = doc(&["a", "b", "c", "d", "e"]);
        let r = d.create_region(f, 1, 1, None, false, vec![]);
        d.move_lines(f, 0, 1, 3);
        assert_eq!(d.card(f).unwrap().lines, s(&["c", "d", "e", "a", "b"]));
        assert_eq!(d.region(r).unwrap().start, 4);
    }

    #[test]
    fn resize_finds_overlaps_but_spares_a_parent() {
        let (mut d, f) = doc(&["a", "b", "c", "d", "e"]);
        let a = d.create_region(f, 0, 2, None, false, vec![]);
        let b = d.create_region(f, 2, 3, None, false, vec![]);
        let parent = d.create_region(f, 0, 4, None, false, vec![]);
        assert_eq!(d.resize_victims(a), vec![b]);
        assert!(!d.resize_victims(a).contains(&parent));
    }

    #[test]
    fn drafts_apply_to_regions_and_refs() {
        let (mut d, f) = doc(&["a", "b", "c"]);
        let r = d.create_region(f, 1, 1, None, false, vec![]);
        let c = d.make_reference(r, 0., 0., 1).unwrap();
        d.region_mut(r).unwrap().talk.draft = Some(s(&["B1", "B2"]));
        assert_eq!(d.draft_stats(c), Some((2, 1)));
        assert!(d.apply_draft(c));
        assert_eq!(d.card(f).unwrap().lines, s(&["a", "B1", "B2", "c"]));
        assert_eq!(d.region(r).unwrap().end, 2);
        assert_eq!(d.card(c).unwrap().lines, s(&["B1", "B2"]));
    }

    #[test]
    fn disk_changes_never_clobber_unsaved_edits() {
        let (mut d, f) = doc(&["a"]);
        assert!(d.apply_disk_text(f, "b\n"));
        assert_eq!(d.card(f).unwrap().lines, s(&["b", ""]));
        d.card_mut(f).unwrap().lines[0] = "mine".into();
        assert!(!d.apply_disk_text(f, "theirs"));
        assert_eq!(d.card(f).unwrap().lines[0], "mine");
    }

    #[test]
    fn project_access_stays_inside_the_project() {
        let base = std::env::temp_dir().join(format!("cc-test-{}", std::process::id()));
        let root = base.join("proj");
        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::write(root.join("a.txt"), "hi").unwrap();
        fs::write(root.join("node_modules/x"), "no").unwrap();
        fs::write(base.join("secret"), "s").unwrap();
        let p = Project::new(root, vec!["node_modules".into()]).unwrap();
        assert_eq!(p.tree(), ["a.txt"]);
        assert_eq!(p.read("a.txt").as_deref(), Some("hi"));
        assert_eq!(p.read("../secret"), None);
        assert!(p.write("../evil", "x").is_err());
        assert!(!base.join("evil").exists());
        fs::remove_dir_all(base).unwrap();
    }
}
