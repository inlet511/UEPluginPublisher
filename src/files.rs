// Copyright 2026 Ken An. All Rights Reserved.

use anyhow::{bail, Context, Result};
use globset::{GlobBuilder, GlobSetBuilder};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

pub fn canonical(path: &Path) -> Result<PathBuf> {
    let path = fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let text = path.to_str().context("A path contains invalid Unicode")?;
        if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
            return Ok(PathBuf::from(format!("\\\\{rest}")));
        }
        if let Some(rest) = text.strip_prefix("\\\\?\\") {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(path)
}

/// Resolve symlinks in existing ancestors, even when the final output doesn't exist.
pub fn future_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut clean = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            other => clean.push(other.as_os_str()),
        }
    }
    let mut ancestor = clean.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        missing.push(
            ancestor
                .file_name()
                .context("Invalid output directory")?
                .to_os_string(),
        );
        ancestor = ancestor
            .parent()
            .context("Output has no existing ancestor")?;
    }
    let mut resolved = canonical(ancestor)?;
    for part in missing.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

pub fn overlaps(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        let a = PathBuf::from(a.to_string_lossy().to_lowercase());
        let b = PathBuf::from(b.to_string_lossy().to_lowercase());
        a.starts_with(&b) || b.starts_with(&a)
    }
    #[cfg(not(windows))]
    {
        a.starts_with(b) || b.starts_with(a)
    }
}

pub fn slash(path: &Path) -> Result<String> {
    Ok(path
        .to_str()
        .context("Non-Unicode archive path")?
        .replace('\\', "/"))
}

pub fn read_json(path: &Path) -> Result<serde_json::Value> {
    let text = fs::read_to_string(path).with_context(|| format!("Reading {}", path.display()))?;
    Ok(serde_json::from_str(text.trim_start_matches('\u{feff}'))?)
}

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string_pretty(value)?)?;
    Ok(())
}

pub fn descriptor(path: &Path) -> Result<PathBuf> {
    let path = canonical(path).with_context(|| format!("Plugin not found: {}", path.display()))?;
    if path.is_file() && path.extension().is_some_and(|x| x == "uplugin") {
        return Ok(path);
    }
    if !path.is_dir() {
        bail!(
            "Expected a .uplugin file or plugin directory: {}",
            path.display()
        );
    }
    let mut found = Vec::new();
    for entry in fs::read_dir(&path)? {
        let path = entry?.path();
        if path.extension().is_some_and(|x| x == "uplugin") && path.is_file() {
            found.push(path);
        }
    }
    if found.len() != 1 {
        bail!(
            "Expected exactly one .uplugin in {}, found {}",
            path.display(),
            found.len()
        );
    }
    Ok(found.remove(0))
}

fn omitted(path: &Path) -> bool {
    let parts: Vec<_> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    let root_skip = [
        "binaries",
        "build",
        "intermediate",
        "saved",
        "deriveddatacache",
        "hostproject",
        "target",
    ];
    parts
        .first()
        .is_some_and(|p| root_skip.contains(&p.as_str()))
        || parts
            .iter()
            .any(|p| p.starts_with('.') || p == "__pycache__" || p == "node_modules")
        || path.extension().is_some_and(|e| {
            matches!(
                e.to_string_lossy().to_lowercase().as_str(),
                "pyc" | "pyo" | "sln" | "suo" | "tmp" | "bak" | "log"
            )
        })
}

pub fn linked(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x400 != 0; // Reparse point, including junctions.
    }
    #[cfg(not(windows))]
    false
}

/// Deterministic, conservative snapshot. No symlinks/junctions are followed.
pub fn source_files(root: &Path, excludes: &[String]) -> Result<Vec<PathBuf>> {
    let mut globs = GlobSetBuilder::new();
    for pattern in excludes {
        if pattern.starts_with('/')
            || pattern.contains('\\')
            || pattern.split('/').any(|c| c == "..")
        {
            bail!("Excludes must be plugin-relative forward-slash globs: {pattern}");
        }
        globs.add(GlobBuilder::new(pattern).literal_separator(true).build()?);
    }
    let globs = globs.build()?;
    let mut result = Vec::new();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let relative = e.path().strip_prefix(root).unwrap_or(e.path());
            !omitted(relative) && !globs.is_match(relative)
        });
    for entry in walker {
        let entry = entry?;
        if entry.depth() == 0 {
            continue;
        }
        if linked(&fs::symlink_metadata(entry.path())?) {
            bail!(
                "Symlinks/junctions must be replaced with real release files: {}",
                entry.path().display()
            );
        }
        if entry.file_type().is_file() {
            result.push(entry.path().strip_prefix(root)?.to_path_buf());
        }
    }
    result.sort();
    Ok(result)
}

pub fn hash(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[derive(Serialize)]
pub struct FileRecord {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

pub fn manifest(root: &Path, paths: &[PathBuf]) -> Result<Vec<FileRecord>> {
    paths
        .iter()
        .map(|p| {
            Ok(FileRecord {
                path: slash(p)?,
                bytes: fs::metadata(root.join(p))?.len(),
                sha256: hash(&root.join(p))?,
            })
        })
        .collect()
}

pub fn copy_snapshot(root: &Path, destination: &Path, paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        let source = root.join(path);
        if linked(&fs::symlink_metadata(&source)?) {
            bail!("Source became a link: {}", source.display());
        }
        let target = destination.join(path);
        fs::create_dir_all(target.parent().context("No target parent")?)?;
        fs::copy(&source, &target)?;
        if hash(&source)? != hash(&target)? {
            bail!("Source changed while copying: {}", source.display());
        }
    }
    Ok(())
}

pub fn all_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if linked(&fs::symlink_metadata(entry.path())?) {
            bail!("Unexpected link: {}", entry.path().display());
        }
        if entry.file_type().is_file() {
            paths.push(entry.path().strip_prefix(root)?.to_path_buf());
        }
    }
    paths.sort();
    Ok(paths)
}

/// ZIP root is the plugin directory's contents, matching UAT's layout.
pub fn archive(root: &Path, paths: &[PathBuf], output: &Path) -> Result<()> {
    let partial = output.with_extension("zip.partial");
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)?;
    let mut zip = ZipWriter::new(file);
    for path in paths {
        let metadata = fs::metadata(root.join(path))?;
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o777
        };
        #[cfg(not(unix))]
        let mode = 0o644;
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(mode)
            .large_file(metadata.len() >= u32::MAX as u64);
        zip.start_file(slash(path)?, options)?;
        std::io::copy(&mut File::open(root.join(path))?, &mut zip)?;
    }
    zip.finish()?.sync_all()?;
    let mut check = ZipArchive::new(File::open(&partial)?)?;
    if check.len() != paths.len() {
        bail!("ZIP file count mismatch");
    }
    // Reading every entry to EOF validates both ZIP contents and their SHA256.
    for (i, path) in paths.iter().enumerate() {
        let mut entry = check.by_index(i)?;
        if entry.name() != slash(path)? {
            bail!("ZIP entry path mismatch");
        }
        let mut digest = Sha256::new();
        std::io::copy(&mut entry, &mut digest)?;
        if format!("{:x}", digest.finalize()) != hash(&root.join(path))? {
            bail!("ZIP hash mismatch: {}", path.display());
        }
    }
    drop(check);
    // Run directories are exclusive; no prior release is replaced.
    if output.exists() {
        bail!("Archive already exists: {}", output.display());
    }
    fs::rename(&partial, output)?;
    fs::write(
        output.with_extension("zip.sha256"),
        format!(
            "{}  {}\n",
            hash(output)?,
            output
                .file_name()
                .context("No ZIP filename")?
                .to_string_lossy()
        ),
    )?;
    Ok(())
}
