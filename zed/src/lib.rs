//! Zed can only start programs for an extension as MCP servers, so this runs the CodeCanvas bridge that way.
//! The bridge finds the app and keeps Zed's CodeCanvas task and shortcut pointing at it.

use serde::Deserialize;
use std::fs;
use zed_extension_api::{self as zed, settings::ContextServerSettings, ContextServerId, Project, Result};

const REPO: &str = "Lightningbulb2/CodeCanvas-zed";

#[derive(Default, Deserialize)]
struct Settings {
    /// Use this bridge instead of downloading one (for example one packaged by Guix).
    bridge_path: Option<String>,
    /// Launch this CodeCanvas instead of the one found automatically.
    app_path: Option<String>,
}

struct CodeCanvas {
    cached: Option<String>,
}

impl CodeCanvas {
    fn bridge(&mut self) -> Result<String> {
        if let Some(p) = self.cached.as_ref().filter(|p| fs::metadata(p).is_ok()) {
            return Ok(p.clone());
        }
        let (os, arch) = zed::current_platform();
        let asset = match (os, arch) {
            (zed::Os::Linux, zed::Architecture::X8664) => "codecanvas-zed-bridge-linux-x86_64",
            (zed::Os::Windows, zed::Architecture::X8664) => "codecanvas-zed-bridge-windows-x86_64.exe",
            (zed::Os::Mac, zed::Architecture::Aarch64) => "codecanvas-zed-bridge-macos-aarch64",
            _ => return Err("No CodeCanvas bridge for this platform; set bridge_path in the codecanvas context server settings.".into()),
        };
        let release = zed::latest_github_release(REPO, zed::GithubReleaseOptions { require_assets: true, pre_release: false })?;
        let file = release.assets.iter().find(|a| a.name == asset).ok_or_else(|| format!("release {} has no {asset}", release.version))?;
        let dir = format!("bridge-{}", release.version);
        let path = format!("{dir}/{asset}");
        if fs::metadata(&path).is_err() {
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            zed::download_file(&file.download_url, &path, zed::DownloadedFileType::Uncompressed)?;
            zed::make_file_executable(&path)?;
        }
        let abs = std::env::current_dir().map_err(|e| e.to_string())?.join(&path).to_string_lossy().into_owned();
        self.cached = Some(abs.clone());
        Ok(abs)
    }
}

impl zed::Extension for CodeCanvas {
    fn new() -> Self {
        Self { cached: None }
    }

    fn context_server_command(&mut self, _id: &ContextServerId, project: &Project) -> Result<zed::Command> {
        let settings: Settings = ContextServerSettings::for_project("codecanvas", project)?
            .settings
            .map(|v| zed::serde_json::from_value(v).map_err(|e| e.to_string()))
            .transpose()?
            .unwrap_or_default();
        let command = match settings.bridge_path {
            Some(p) => p,
            None => self.bridge()?,
        };
        let args = settings.app_path.map(|p| vec!["--app".to_string(), p]).unwrap_or_default();
        Ok(zed::Command { command, args, env: Vec::new() })
    }
}

zed::register_extension!(CodeCanvas);
