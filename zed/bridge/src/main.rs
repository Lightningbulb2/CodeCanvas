//! MCP server (JSON lines on stdio) that Zed starts for the CodeCanvas extension.
//! On start it keeps Zed's task and shortcut pointing at the installed app; it also offers an open_canvas tool.

use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const LABEL: &str = "CodeCanvas: open project";
const KEY: &str = "ctrl-alt-c";
const EXE: &str = if cfg!(windows) { "codecanvas.exe" } else { "codecanvas" };

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

fn find_app(explicit: Option<PathBuf>) -> Option<PathBuf> {
    if explicit.is_some() {
        return explicit.filter(|p| p.is_file());
    }
    if let Some(p) = on_path(EXE) {
        return Some(p);
    }
    let mut known = Vec::new();
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        known.push(PathBuf::from(l).join("Programs").join("CodeCanvas").join(EXE));
    }
    if let Some(h) = std::env::var_os("HOME") {
        known.push(PathBuf::from(h).join(".local").join("bin").join(EXE));
    }
    known.into_iter().find(|p| p.is_file())
}

fn zed_config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Zed"));
    }
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute());
    xdg.or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).map(|c| c.join("zed"))
}

fn task_json(app: &Path) -> String {
    serde_json::to_string_pretty(&json!({
        "label": LABEL, "command": app.to_string_lossy(), "args": ["$ZED_WORKTREE_ROOT", "${ZED_FILE:}"],
        "reveal": "never", "hide": "always", "use_new_terminal": true, "allow_concurrent_runs": true,
    }))
    .unwrap()
}

fn key_json() -> String {
    serde_json::to_string_pretty(&json!({ "context": "Workspace", "bindings": { KEY: ["task::Spawn", { "task_name": LABEL }] } })).unwrap()
}

/// Adds `entry` to a JSON-with-comments array without disturbing the rest. `None` if the text can't be edited safely.
fn insert_entry(text: &str, entry: &str) -> Option<String> {
    let close = text.rfind(']')?;
    let before = text[..close].trim_end();
    let needs_comma = match before.chars().last()? {
        '[' | ',' => false,
        '}' => true,
        _ => return None, // ends in a comment or something unexpected: leave the file alone
    };
    Some(format!("{before}{}\n{entry}\n{}", if needs_comma { "," } else { "" }, &text[close..]))
}

/// Re-points the existing task at `app`. `None` if nothing needs changing.
fn update_command(text: &str, app: &Path) -> Option<String> {
    let label = text.find(LABEL)?;
    // Keys can come in any order, so search the whole object holding the label.
    let start = text[..label].rfind('{')?;
    let end_obj = label + text[label..].find('}')?;
    let key = start + text[start..end_obj].find("\"command\"")?;
    let colon = key + text[key..].find(':')?;
    let open = colon + text[colon..].find('"')?;
    let mut end = open + 1;
    let bytes = text.as_bytes();
    while end < text.len() && bytes[end] != b'"' {
        end += if bytes[end] == b'\\' { 2 } else { 1 };
    }
    let new = serde_json::to_string(&app.to_string_lossy()).unwrap();
    (text[open..=end] != new).then(|| format!("{}{new}{}", &text[..open], &text[end + 1..]))
}

fn write_with_backup(file: &Path, old: &str, new: &str) -> std::io::Result<()> {
    let bak = file.with_extension("json.codecanvas-backup");
    if !bak.exists() {
        fs::write(bak, old)?;
    }
    fs::write(file, new)
}

/// Makes Zed's task and shortcut match the installed app. Safe to run on every start.
fn sync_zed(dir: &Path, app: &Path) -> std::io::Result<Vec<String>> {
    fs::create_dir_all(dir)?;
    let mut log: Vec<String> = Vec::new();
    let tasks = dir.join("tasks.json");
    match fs::read_to_string(&tasks) {
        Err(_) => {
            fs::write(&tasks, format!("[\n{}\n]\n", task_json(app)))?;
            log.push("created tasks.json".into());
        }
        Ok(t) if t.contains(LABEL) => {
            if let Some(n) = update_command(&t, app) {
                write_with_backup(&tasks, &t, &n)?;
                log.push("updated the app path in tasks.json".into());
            }
        }
        Ok(t) => match insert_entry(&t, &task_json(app)) {
            Some(n) => {
                write_with_backup(&tasks, &t, &n)?;
                log.push("added the task to tasks.json".into());
            }
            None => log.push("tasks.json could not be edited safely; left unchanged".into()),
        },
    }
    let keymap = dir.join("keymap.json");
    match fs::read_to_string(&keymap) {
        Err(_) => {
            fs::write(&keymap, format!("[\n{}\n]\n", key_json()))?;
            log.push("created keymap.json".into());
        }
        Ok(t) if t.contains(LABEL) => {}
        Ok(t) if t.contains(&format!("\"{KEY}\"")) => log.push(format!("{KEY} is already bound; add a key for the task yourself")),
        Ok(t) => match insert_entry(&t, &key_json()) {
            Some(n) => {
                write_with_backup(&keymap, &t, &n)?;
                log.push("added the shortcut to keymap.json".into());
            }
            None => log.push("keymap.json could not be edited safely; left unchanged".into()),
        },
    }
    Ok(log)
}

fn open_canvas(app: Option<&Path>, args: &Value) -> Result<String, String> {
    let app = app.ok_or("CodeCanvas is not installed where the extension can find it: put `codecanvas` on PATH or set app_path.")?;
    let mut cmd = Command::new(app);
    let mut opened = Vec::new();
    for k in ["path", "file"] {
        if let Some(v) = args.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()) {
            cmd.arg(v);
            opened.push(v.to_string());
        }
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("could not start {}: {e}", app.display()))?;
    Ok(format!("Opened CodeCanvas {}", opened.join(" ")))
}

fn handle(msg: &Value, app: Option<&Path>) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let result = match msg["method"].as_str()? {
        "initialize" => json!({
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "codecanvas", "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": [{
            "name": "open_canvas",
            "description": "Open a project, and optionally one file in it, on the CodeCanvas canvas.",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string", "description": "Absolute project folder" },
                "file": { "type": "string", "description": "Absolute path of a file to place on the canvas" },
            }, "required": ["path"] },
        }] }),
        "tools/call" if msg["params"]["name"] == "open_canvas" => {
            let (text, err) = match open_canvas(app, &msg["params"]["arguments"]) {
                Ok(t) => (t, false),
                Err(t) => (t, true),
            };
            json!({ "content": [{ "type": "text", "text": text }], "isError": err })
        }
        m => return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("unknown method {m}") } })),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut explicit = None;
    while let Some(a) = args.next() {
        if a == "--app" {
            explicit = args.next().map(PathBuf::from);
        }
    }
    let app = find_app(explicit);
    match (&app, zed_config_dir()) {
        (Some(app), Some(dir)) => match sync_zed(&dir, app) {
            Ok(log) => log.iter().for_each(|l| eprintln!("codecanvas: {l}")),
            Err(e) => eprintln!("codecanvas: could not update the Zed config: {e}"),
        },
        _ => eprintln!("codecanvas: app not found; the Zed task was not set up"),
    }
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(reply) = handle(&msg, app.as_deref()) {
            if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cc-bridge-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn creates_then_stays_put() {
        let d = tmp("create");
        sync_zed(&d, Path::new("/opt/a b/codecanvas")).unwrap();
        let t: Value = serde_json::from_str(&fs::read_to_string(d.join("tasks.json")).unwrap()).unwrap();
        assert_eq!(t[0]["command"], "/opt/a b/codecanvas");
        let k: Value = serde_json::from_str(&fs::read_to_string(d.join("keymap.json")).unwrap()).unwrap();
        assert_eq!(k[0]["bindings"][KEY][1]["task_name"], LABEL);
        assert!(sync_zed(&d, Path::new("/opt/a b/codecanvas")).unwrap().is_empty());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn follows_the_app_when_it_moves_and_handles_windows_paths() {
        let d = tmp("move");
        sync_zed(&d, Path::new(r"C:\old\codecanvas.exe")).unwrap();
        sync_zed(&d, Path::new(r"D:\new place\codecanvas.exe")).unwrap();
        let t: Value = serde_json::from_str(&fs::read_to_string(d.join("tasks.json")).unwrap()).unwrap();
        assert_eq!(t[0]["command"], r"D:\new place\codecanvas.exe");
        assert_eq!(t.as_array().unwrap().len(), 1);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn keeps_existing_entries_and_comments() {
        let d = tmp("merge");
        fs::create_dir_all(&d).unwrap();
        let mine = "// my tasks\n[\n  { \"label\": \"build\", \"command\": \"make\" }\n]\n";
        fs::write(d.join("tasks.json"), mine).unwrap();
        sync_zed(&d, Path::new("/x/codecanvas")).unwrap();
        let out = fs::read_to_string(d.join("tasks.json")).unwrap();
        assert!(out.starts_with("// my tasks") && out.contains("\"build\"") && out.contains(LABEL));
        let clean = out.replace("// my tasks", "");
        assert_eq!(serde_json::from_str::<Value>(&clean).unwrap().as_array().unwrap().len(), 2);
        assert_eq!(fs::read_to_string(d.join("tasks.json.codecanvas-backup")).unwrap(), mine);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn never_edits_a_file_it_cannot_parse_safely() {
        let d = tmp("unsafe");
        fs::create_dir_all(&d).unwrap();
        let mine = "[\n  { \"label\": \"a\" } // trailing note\n]\n";
        fs::write(d.join("tasks.json"), mine).unwrap();
        sync_zed(&d, Path::new("/x/codecanvas")).unwrap();
        assert_eq!(fs::read_to_string(d.join("tasks.json")).unwrap(), mine);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn does_not_take_over_a_used_key() {
        let d = tmp("key");
        fs::create_dir_all(&d).unwrap();
        let mine = "[\n  { \"bindings\": { \"ctrl-alt-c\": \"x\" } }\n]\n";
        fs::write(d.join("keymap.json"), mine).unwrap();
        sync_zed(&d, Path::new("/x/codecanvas")).unwrap();
        assert_eq!(fs::read_to_string(d.join("keymap.json")).unwrap(), mine);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn answers_mcp() {
        let r = handle(&json!({"id":1,"method":"tools/list"}), None).unwrap();
        assert_eq!(r["result"]["tools"][0]["name"], "open_canvas");
        assert!(handle(&json!({"method":"notifications/initialized"}), None).is_none());
        let r = handle(&json!({"id":2,"method":"tools/call","params":{"name":"open_canvas","arguments":{"path":"/x"}}}), None).unwrap();
        assert_eq!(r["result"]["isError"], true);
    }
}
