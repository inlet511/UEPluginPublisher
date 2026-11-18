// Copyright 2026 Ken An. All Rights Reserved.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

fn yes() -> bool {
    true
}
pub fn default_parallel_actions() -> u32 {
    2
}
fn host_platforms() -> Vec<String> {
    vec![if cfg!(windows) {
        "Win64"
    } else if cfg!(target_os = "macos") {
        "Mac"
    } else {
        "Linux"
    }
    .into()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub output: PathBuf,
    /// An empty list means all locally discovered UE5 installations.
    #[serde(default)]
    pub engines: Vec<PathBuf>,
    pub plugins: Vec<Plugin>,
    #[serde(default = "yes")]
    pub build: bool,
    #[serde(default = "yes")]
    pub binary_archive: bool,
    /// Child-process-only UBT controls; never modifies global UE settings.
    #[serde(default)]
    pub low_memory_build: bool,
    #[serde(default = "default_parallel_actions")]
    pub max_parallel_actions: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plugin {
    /// Either a .uplugin file or a directory containing exactly one descriptor.
    pub path: PathBuf,
    #[serde(default = "host_platforms")]
    pub platforms: Vec<String>,
    /// Optional engine selection for this plugin; otherwise use Config.engines.
    #[serde(default)]
    pub engines: Vec<PathBuf>,
    /// Paths/globs relative to the plugin root, using forward slashes.
    #[serde(default)]
    pub exclude: Vec<String>,
}

pub fn load(path: &Path) -> Result<Config> {
    let path =
        fs::canonicalize(path).with_context(|| format!("Config not found: {}", path.display()))?;
    let text = fs::read_to_string(&path)?;
    let mut config: Config = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
    let base = path.parent().context("Config has no parent directory")?;
    let resolve = |p: &mut PathBuf| {
        if p.is_relative() {
            *p = base.join(&*p);
        }
    };
    resolve(&mut config.output);
    for engine in &mut config.engines {
        resolve(engine);
    }
    for plugin in &mut config.plugins {
        resolve(&mut plugin.path);
        for engine in &mut plugin.engines {
            resolve(engine);
        }
    }
    validate(&config)?;
    Ok(config)
}

pub fn validate(config: &Config) -> Result<()> {
    if config.plugins.is_empty() {
        bail!("请至少添加一个插件 / The profile must contain at least one plugin");
    }
    if config.output.as_os_str().is_empty() {
        bail!("请选择输出目录 / Output directory is empty");
    }
    if !(1..=64).contains(&config.max_parallel_actions) {
        bail!("编译并发上限须在 1–64 之间 / max_parallel_actions must be 1–64");
    }
    for plugin in &config.plugins {
        if plugin.path.as_os_str().is_empty() {
            bail!("Plugin path is empty");
        }
        if plugin.platforms.is_empty() {
            bail!("No target platforms for {}", plugin.path.display());
        }
        for platform in &plugin.platforms {
            if platform.is_empty() || !platform.chars().all(|c| c.is_ascii_alphanumeric()) {
                bail!("Invalid target platform: {platform}");
            }
        }
    }
    Ok(())
}

/// Keep a previous profile as a backup and publish the new one by rename.
pub fn save(path: &Path, config: &Config) -> Result<()> {
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};
    validate(config)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let staged = path.with_extension(format!("json.{stamp}.tmp"));
    let backup = path.with_extension(format!("json.{stamp}.bak"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    writeln!(file, "{}", serde_json::to_string_pretty(config)?)?;
    file.sync_all()?;
    drop(file);
    let existed = path.exists();
    if existed {
        fs::rename(path, &backup)?;
    }
    if let Err(error) = fs::rename(&staged, path) {
        if existed {
            let _ = fs::rename(&backup, path);
        }
        return Err(error.into());
    }
    Ok(())
}

pub fn write_new(path: &Path, config: &Config) -> Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| {
            format!(
                "Cannot create profile (existing profiles are preserved): {}",
                path.display()
            )
        })?;
    writeln!(file, "{}", serde_json::to_string_pretty(config)?)?;
    Ok(())
}
