// Copyright 2026 Ken An. All Rights Reserved.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct Engine {
    pub root: PathBuf,
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub uat: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Version {
    major_version: u32,
    minor_version: u32,
    patch_version: u32,
}

impl Engine {
    pub fn read(path: &Path) -> Result<Self> {
        // Accept the installation root, Engine folder, or the RunUAT script itself.
        let mut root = crate::files::canonical(path)
            .with_context(|| format!("Engine path not found: {}", path.display()))?;
        if root.is_file() {
            root = root
                .parent()
                .context("Invalid engine script path")?
                .to_path_buf();
        }
        while !root.join("Engine/Build/Build.version").is_file() {
            if !root.pop() {
                bail!(
                    "Cannot find Engine/Build/Build.version from {}",
                    path.display()
                );
            }
        }
        let version: Version = serde_json::from_str(
            fs::read_to_string(root.join("Engine/Build/Build.version"))?
                .trim_start_matches('\u{feff}'),
        )?;
        if version.major_version != 5 {
            bail!("Only UE5 is supported: {}", root.display());
        }
        let uat = root.join(if cfg!(windows) {
            "Engine/Build/BatchFiles/RunUAT.bat"
        } else {
            "Engine/Build/BatchFiles/RunUAT.sh"
        });
        if !uat.is_file() {
            bail!("Missing RunUAT: {}", uat.display());
        }
        Ok(Self {
            root,
            major: version.major_version,
            minor: version.minor_version,
            patch: version.patch_version,
            uat,
        })
    }

    pub fn label(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }

    pub fn descriptor_version(&self) -> String {
        // Plugin descriptors conventionally target the base minor release.
        format!("{}.{}.0", self.major, self.minor)
    }
}

fn add_children(root: &Path, candidates: &mut BTreeSet<PathBuf>) {
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("UE_5") {
                candidates.insert(entry.path());
            }
        }
    }
}

#[cfg(windows)]
fn registry(candidates: &mut BTreeSet<PathBuf>) {
    use winreg::{enums::*, RegKey};
    let user = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = user.open_subkey("Software\\Epic Games\\Unreal Engine\\Builds") {
        for (name, _) in key.enum_values().flatten() {
            if let Ok(path) = key.get_value::<String, _>(&name) {
                candidates.insert(PathBuf::from(path));
            }
        }
    }
    let machine = RegKey::predef(HKEY_LOCAL_MACHINE);
    for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
        if let Ok(key) =
            machine.open_subkey_with_flags("SOFTWARE\\EpicGames\\Unreal Engine", KEY_READ | view)
        {
            for name in key.enum_keys().flatten() {
                if let Ok(version) = key.open_subkey_with_flags(name, KEY_READ | view) {
                    if let Ok(path) = version.get_value::<String, _>("InstalledDirectory") {
                        candidates.insert(PathBuf::from(path));
                    }
                }
            }
        }
    }
}

pub fn discover(extra_roots: &[PathBuf]) -> Vec<Engine> {
    let mut candidates = BTreeSet::new();
    for name in ["UE_ENGINE_ROOT", "UE_ROOT", "UNREAL_ENGINE_ROOT"] {
        if let Some(path) = env::var_os(name) {
            candidates.insert(PathBuf::from(path));
        }
    }
    #[cfg(windows)]
    {
        registry(&mut candidates);
        if let Some(data) = env::var_os("ProgramData") {
            let data = PathBuf::from(data);
            let launcher = data.join("Epic/UnrealEngineLauncher/LauncherInstalled.dat");
            if let Ok(text) = fs::read_to_string(launcher) {
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    if let Some(list) = value["InstallationList"].as_array() {
                        for item in list {
                            if let Some(path) = item["InstallLocation"].as_str() {
                                candidates.insert(PathBuf::from(path));
                            }
                        }
                    }
                }
            }
            let manifests = data.join("Epic/EpicGamesLauncher/Data/Manifests");
            if let Ok(entries) = fs::read_dir(manifests) {
                for entry in entries
                    .flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "item"))
                {
                    if let Ok(text) = fs::read_to_string(entry.path()) {
                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                            if let Some(path) = value["InstallLocation"].as_str() {
                                candidates.insert(PathBuf::from(path));
                            }
                        }
                    }
                }
            }
        }
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(path) = env::var_os(var) {
                add_children(&PathBuf::from(path).join("Epic Games"), &mut candidates);
            }
        }
    }
    #[cfg(target_os = "macos")]
    add_children(Path::new("/Users/Shared/Epic Games"), &mut candidates);
    for root in extra_roots {
        candidates.insert(root.clone());
        add_children(root, &mut candidates);
    }
    let mut seen = BTreeSet::new();
    let mut engines: Vec<_> = candidates
        .into_iter()
        .filter_map(|p| Engine::read(&p).ok())
        .filter(|e| seen.insert(e.root.clone()))
        .collect();
    engines.sort_by(|a, b| {
        (b.major, b.minor, b.patch)
            .cmp(&(a.major, a.minor, a.patch))
            .then(a.root.cmp(&b.root))
    });
    engines
}
