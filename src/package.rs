// Copyright 2026 Ken An. All Rights Reserved.

use crate::{
    config,
    engines::{self, Engine},
    files,
    progress::Reporter,
};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

struct Plan<'a> {
    plugin: &'a config::Plugin,
    descriptor: PathBuf,
    root: PathBuf,
    name: String,
    engine: Engine,
    paths: Vec<PathBuf>,
}

fn code(path: &Path) -> bool {
    path.extension().is_some_and(|e| {
        matches!(
            e.to_string_lossy().to_lowercase().as_str(),
            "h" | "hpp"
                | "hh"
                | "c"
                | "cpp"
                | "cc"
                | "cxx"
                | "inl"
                | "cs"
                | "usf"
                | "ush"
                | "py"
                | "rs"
        )
    })
}

fn audit(root: &Path, name: &str, paths: &[PathBuf]) -> Result<()> {
    let mut issues = Vec::new();
    for path in paths {
        let relative = files::slash(path)?;
        if format!("{name}/{relative}").chars().count() > 170 {
            issues.push(format!(
                "Path exceeds 170 characters including plugin folder: {relative}"
            ));
        }
        if path.extension().is_some_and(|e| e == "py") && !relative.starts_with("Content/Python/") {
            issues.push(format!("Python must be inside Content/Python: {relative}"));
        }
        if code(path) {
            let text = fs::read_to_string(root.join(path))
                .with_context(|| format!("Code must be readable UTF-8: {relative}"))?;
            let first = text
                .trim_start_matches('\u{feff}')
                .lines()
                .take(8)
                .collect::<Vec<_>>()
                .join("\n")
                .to_lowercase();
            let year = first.split(|c: char| !c.is_ascii_digit()).any(|n| {
                n.len() == 4 && n.parse::<u32>().is_ok_and(|y| (1900..=2100).contains(&y))
            });
            let comment = first.trim_start().starts_with("//")
                || first.trim_start().starts_with("/*")
                || first.trim_start().starts_with('#');
            if !comment || !first.contains("copyright") || !year {
                issues.push(format!(
                    "Missing opening copyright comment with a publication year: {relative}"
                ));
            }
        }
    }
    if !issues.is_empty() {
        bail!("Release preflight failed:\n{}", issues.join("\n"));
    }
    Ok(())
}

fn checked_descriptor(path: &Path, platforms: &[String]) -> Result<Value> {
    let descriptor = files::read_json(path)?;
    if !descriptor.is_object() || !descriptor["FileVersion"].is_number() {
        bail!("Invalid plugin descriptor: {}", path.display());
    }
    if descriptor["CreatedBy"]
        .as_str()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        bail!("Set the publisher's CreatedBy field in {}", path.display());
    }
    if let Some(supported) = descriptor["SupportedTargetPlatforms"].as_array() {
        for platform in platforms {
            if !supported
                .iter()
                .any(|p| p.as_str() == Some(platform.as_str()))
            {
                bail!(
                    "Platform {platform} is not in SupportedTargetPlatforms: {}",
                    path.display()
                );
            }
        }
    }
    if let Some(modules) = descriptor.get("Modules") {
        let modules = modules.as_array().context("Modules must be an array")?;
        for module in modules {
            if !module.is_object()
                || module["Name"].as_str().is_none()
                || module["Type"].as_str().is_none()
            {
                bail!("Every module needs Name and Type: {}", path.display());
            }
            for field in [
                "PlatformAllowList",
                "PlatformDenyList",
                "WhitelistPlatforms",
                "BlacklistPlatforms",
            ] {
                if let Some(value) = module.get(field) {
                    if !value
                        .as_array()
                        .is_some_and(|a| a.iter().all(Value::is_string))
                    {
                        bail!("Module {field} must be a string array");
                    }
                }
            }
        }
    }
    Ok(descriptor)
}

fn plans(config: &config::Config) -> Result<Vec<Plan<'_>>> {
    let output = files::future_path(&config.output)?;
    let discovered = engines::discover(&[]);
    let mut plans = Vec::new();
    for plugin in &config.plugins {
        let descriptor = files::descriptor(&plugin.path)?;
        let root = descriptor
            .parent()
            .context("Plugin has no parent")?
            .to_path_buf();
        if files::overlaps(&root, &output) {
            bail!(
                "Output and plugin source cannot contain each other: {}",
                root.display()
            );
        }
        let name = descriptor
            .file_stem()
            .context("No plugin name")?
            .to_str()
            .context("Non-Unicode plugin name")?
            .to_string();
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            bail!("Use an ASCII alphanumeric/underscore plugin descriptor name: {name}");
        }
        checked_descriptor(&descriptor, &plugin.platforms)?;
        let paths = files::source_files(&root, &plugin.exclude)?;
        let descriptor_name =
            PathBuf::from(descriptor.file_name().context("No descriptor filename")?);
        if !paths.contains(&descriptor_name) {
            bail!("Do not exclude the plugin descriptor");
        }
        if paths
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "uplugin"))
            .count()
            != 1
        {
            bail!("A release must contain exactly one plugin descriptor");
        }
        audit(&root, &name, &paths)?;
        let selection = if plugin.engines.is_empty() {
            &config.engines
        } else {
            &plugin.engines
        };
        let selected: Vec<Engine> = if selection.is_empty() {
            discovered.clone()
        } else {
            selection
                .iter()
                .map(|p| Engine::read(p))
                .collect::<Result<_>>()?
        };
        if selected.is_empty() {
            bail!("No UE5 found. Run the wizard or add explicit engine paths to the profile.");
        }
        let mut seen = BTreeSet::new();
        for mut engine in selected {
            engine.root = files::canonical(&engine.root)?;
            engine.uat = files::canonical(&engine.uat)?;
            if files::overlaps(&engine.root, &output) {
                bail!("Output must be outside engine installations");
            }
            if seen.insert(engine.root.clone()) {
                plans.push(Plan {
                    plugin,
                    descriptor: descriptor.clone(),
                    root: root.clone(),
                    name: name.clone(),
                    engine,
                    paths: paths.clone(),
                });
            }
        }
    }
    Ok(plans)
}

fn prepare_descriptor(plan: &Plan<'_>, snapshot: &Path) -> Result<()> {
    let path = snapshot.join(
        plan.descriptor
            .file_name()
            .context("No descriptor filename")?,
    );
    let mut descriptor = checked_descriptor(&path, &plan.plugin.platforms)?;
    descriptor["EngineVersion"] = json!(plan.engine.descriptor_version());
    descriptor["Installed"] = json!(false);
    if let Some(modules) = descriptor.get_mut("Modules").and_then(Value::as_array_mut) {
        for module in modules {
            // UAT always serializes this default, including when the input omits it.
            if module.get("LoadingPhase").is_none() {
                module["LoadingPhase"] = json!("Default");
            }
            // Translate legacy UE4 keys but preserve each module's actual restrictions.
            for (old, new) in [
                ("WhitelistPlatforms", "PlatformAllowList"),
                ("BlacklistPlatforms", "PlatformDenyList"),
            ] {
                if let Some(value) = module
                    .as_object_mut()
                    .context("Invalid module")?
                    .remove(old)
                {
                    if module.get(new).is_none() {
                        module[new] = value;
                    }
                }
            }
            if module.get("PlatformAllowList").is_none() && module.get("PlatformDenyList").is_none()
            {
                module["PlatformAllowList"] = json!(plan.plugin.platforms);
            }
        }
    }
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(&descriptor)?),
    )?;
    Ok(())
}

fn prepare_filter(snapshot: &Path) -> Result<()> {
    let config = snapshot.join("Config");
    fs::create_dir_all(&config)?;
    let path = config.join("FilterPlugin.ini");
    let original = if path.is_file() {
        fs::read_to_string(&path)?
    } else {
        String::new()
    };
    // Publisher rules come last so their exclusions cannot be silently overridden.
    let mut filter = String::from("[FilterPlugin]\n");
    let mut extras = Vec::new();
    for entry in fs::read_dir(snapshot)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .to_str()
            .context("Non-Unicode filter path")?
            .to_string();
        if name
            .chars()
            .any(|c| matches!(c, '\n' | '\r' | '*' | '?' | '[' | ']' | ';'))
        {
            bail!("Cannot express custom path in FilterPlugin.ini: {name}");
        }
        if name.ends_with(".uplugin") || name == "Source" {
            continue;
        }
        extras.push(if entry.file_type()?.is_dir() {
            format!("/{name}/...")
        } else {
            format!("/{name}")
        });
    }
    extras.sort();
    for rule in extras {
        filter.push_str(&rule);
        filter.push('\n');
    }
    filter.push('\n');
    filter.push_str(original.trim_start_matches('\u{feff}'));
    fs::write(path, filter)?;
    Ok(())
}

fn batch_safe(path: &Path) -> Result<String> {
    let text = path.to_str().context("Non-Unicode process path")?;
    if text.chars().any(|c| {
        matches!(
            c,
            '"' | '%' | '!' | '&' | '|' | '<' | '>' | '^' | '\r' | '\n'
        )
    }) {
        bail!(
            "RunUAT batch paths cannot contain shell metacharacters: {}",
            path.display()
        );
    }
    Ok(text.to_string())
}

fn check_force_stop(reporter: &Reporter) -> Result<()> {
    if reporter.force_stopping() {
        bail!("任务已强制终止；本次未完成产物不可提交，日志与已完成任务保留。");
    }
    Ok(())
}

fn ubt_environment(plan: &Plan<'_>, config: &config::Config) -> Result<Option<String>> {
    if !config.low_memory_build {
        return Ok(None);
    }
    // BuildPlugin does not forward arbitrary -UBTArgs in all UE releases.
    // Check this engine's actual implementation instead of silently ignoring limits.
    let source = plan
        .engine
        .root
        .join("Engine/Source/Programs/UnrealBuildTool/UnrealBuildTool.cs");
    let source = fs::read_to_string(&source).with_context(|| {
        format!(
            "无法确认 UE {} 对低内存模式的支持；请关闭该选项，或使用包含 UBT 源码的引擎安装。",
            plan.engine.label()
        )
    })?;
    if !source.contains("GetEnvironmentVariable(\"UBT_EXTRA_ARGS\")") {
        bail!("UE {} 未检测到 UBT_EXTRA_ARGS 支持；请关闭低内存模式，并在该引擎的 UBT 配置中手动限制并发。", plan.engine.label());
    }
    let inherited = match std::env::var("UBT_EXTRA_ARGS") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => String::new(),
        Err(error) => return Err(error).context("UBT_EXTRA_ARGS 环境变量无法读取"),
    };
    if inherited
        .to_ascii_lowercase()
        .contains("-maxparallelactions")
        || inherited
            .split_whitespace()
            .any(|arg| arg.trim_matches('"').eq_ignore_ascii_case("-UBA"))
    {
        bail!("已有 UBT_EXTRA_ARGS 含并发或 UBA 启用参数，与低内存模式冲突；请先移除冲突参数。");
    }
    // Use the local parallel executor so the concurrency cap is effective.
    Ok(Some(format!(
        "{inherited} -NoUBA -NoXGE -NoFASTBuild -NoSNDBS -MaxParallelActions={}",
        config.max_parallel_actions
    )))
}

fn terminate_build_tree(child: &mut std::process::Child) -> Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    let pid = child.id().to_string();
    #[cfg(windows)]
    let result = {
        use std::os::windows::process::CommandExt;
        Command::new("taskkill.exe")
            .args(["/PID", &pid, "/T", "/F"])
            .creation_flags(0x08000000)
            .output()
    };
    #[cfg(not(windows))]
    let result = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .output();
    let result = result.context("无法启动进程树终止命令")?;
    if !result.status.success() && child.try_wait()?.is_none() {
        bail!(
            "终止进程树失败：{} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    Ok(())
}

fn build(
    plan: &Plan<'_>,
    snapshot: &Path,
    output: &Path,
    log_path: &Path,
    reporter: &Reporter,
    config: &config::Config,
) -> Result<Vec<String>> {
    check_force_stop(reporter)?;
    if output.exists() {
        bail!("UAT destination must be a fresh directory");
    }
    let descriptor = snapshot.join(
        plan.descriptor
            .file_name()
            .context("No descriptor filename")?,
    );
    let arguments = vec![
        "BuildPlugin".to_string(),
        format!("-Plugin={}", batch_safe(&descriptor)?),
        format!("-Package={}", batch_safe(output)?),
        format!("-TargetPlatforms={}", plan.plugin.platforms.join("+")),
        "-Rocket".into(),
        "-StrictIncludes".into(),
    ];
    let mut command;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let uat = batch_safe(&plan.engine.uat)?;
        let args = arguments
            .iter()
            .map(|a| format!("\"{a}\""))
            .collect::<Vec<_>>()
            .join(" ");
        command = Command::new("cmd.exe");
        command
            .args(["/d", "/v:off", "/s", "/c"])
            .creation_flags(0x08000000)
            .raw_arg(format!("\"\"{uat}\" {args}\""));
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        command = Command::new("/bin/bash");
        command
            .arg(&plan.engine.uat)
            .args(&arguments)
            .process_group(0);
    }
    if let Some(extra) = ubt_environment(plan, config)? {
        command.env("UBT_EXTRA_ARGS", &extra);
        reporter.log(format!(
            "低内存模式：最多 {} 个编译任务，禁用 UBA（仅本次构建）。",
            config.max_parallel_actions
        ));
    }
    check_force_stop(reporter)?;
    reporter.log("严格头文件检查已启用：关闭 PCH / Unity 编译，耗时通常高于编辑器内增量编译。");
    // Both streams go directly to one log. The child cannot block on full pipes,
    // and the user sees a live stream by tailing the same file below.
    let log = File::options()
        .create_new(true)
        .read(true)
        .write(true)
        .open(log_path)?;
    command
        .current_dir(&plan.engine.root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log.try_clone()?));
    reporter.status(format!(
        "正在编译验证 {} / UE {}。日志：{}",
        plan.name,
        plan.engine.label(),
        log_path.display()
    ));
    let mut reader = BufReader::new(File::open(log_path)?);
    let mut child = command.spawn().context("Cannot start RunUAT")?;
    let mut line = Vec::new();
    let mut was_forced = false;
    let mut memory_warning_logged = false;
    let status = (|| -> Result<std::process::ExitStatus> {
        loop {
            if reporter.force_stopping() && !was_forced {
                reporter.status(format!("正在终止 UAT 进程树（PID {}）…", child.id()));
                match terminate_build_tree(&mut child) {
                    Ok(()) => {
                        was_forced = true;
                        // Keep the complete log on disk; do not delay cancellation by
                        // replaying a potentially very large unread log into the GUI.
                        return child.wait().context("等待已终止的 UAT 退出失败");
                    }
                    Err(error) => {
                        reporter
                            .status(format!("{error:#}；当前任务仍在运行，可再次点击强制终止。"));
                        reporter
                            .force_stop
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            line.clear();
            if reader.read_until(b'\n', &mut line)? > 0 {
                let text = String::from_utf8_lossy(&line).trim_end().to_string();
                if !memory_warning_logged && text.contains("Low on memory") {
                    memory_warning_logged = true;
                    reporter.status("检测到编译内存不足。可强制终止后启用低内存模式重试，并关闭不需要的编辑器或其他高内存程序。");
                }
                reporter.log(text);
                continue;
            }
            if let Some(status) = child.try_wait()? {
                // Flush the final bytes written before process exit.
                loop {
                    line.clear();
                    if reader.read_until(b'\n', &mut line)? == 0 {
                        break;
                    }
                    reporter.log(String::from_utf8_lossy(&line).trim_end().to_string());
                }
                return Ok(status);
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    })();
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            reporter.status("日志读取异常，正在等待当前 UAT 退出；仍可强制终止。");
            // Do not mark the GUI idle while a build is still running.
            loop {
                if reporter.force_stopping() {
                    if let Err(stop_error) = terminate_build_tree(&mut child) {
                        reporter.status(format!("{stop_error:#}；可再次点击强制终止。"));
                        reporter
                            .force_stop
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(150)),
                    Err(_) => {
                        let _ = child.wait();
                        break;
                    }
                }
            }
            return Err(error);
        }
    };
    if was_forced {
        bail!(
            "本次编译已强制终止；未生成通过验证的源码包。日志：{}",
            log_path.display()
        );
    }
    if !status.success() {
        bail!("BuildPlugin failed ({status}); see {}", log_path.display());
    }
    Ok(arguments)
}

fn verify_uat(snapshot: &Path, output: &Path, paths: &[PathBuf], descriptor: &Path) -> Result<()> {
    for path in paths {
        let target = output.join(path);
        if !target.is_file() {
            bail!(
                "UAT filtered out a required file: {}. Check Config/FilterPlugin.ini.",
                path.display()
            );
        }
        if path != descriptor && files::hash(&snapshot.join(path))? != files::hash(&target)? {
            bail!("UAT package changed a required file: {}", path.display());
        }
    }
    let built = files::read_json(&output.join(descriptor))?;
    let source = files::read_json(&snapshot.join(descriptor))?;
    for field in ["EngineVersion", "Modules"] {
        if built[field] != source[field] {
            bail!("UAT changed descriptor field {field}");
        }
    }
    Ok(())
}

fn one(
    plan: &Plan<'_>,
    folder: &Path,
    config: &config::Config,
    source_only: bool,
    reporter: &Reporter,
) -> Result<Value> {
    check_force_stop(reporter)?;
    let build_enabled = config.build && !source_only;
    reporter.status(format!(
        "{} / UE {} · 复制源码快照与生成清单",
        plan.name,
        plan.engine.label()
    ));
    let snapshot = folder.join("Snapshot").join(&plan.name);
    fs::create_dir_all(&snapshot)?;
    // The same source bytes must be present before and after snapshot creation.
    let before = files::manifest(&plan.root, &plan.paths)?;
    files::copy_snapshot(&plan.root, &snapshot, &plan.paths)?;
    check_force_stop(reporter)?;
    let after = files::manifest(&plan.root, &plan.paths)?;
    if serde_json::to_value(&before)? != serde_json::to_value(&after)? {
        bail!("Source changed while snapshotting; repeat the release with editing paused");
    }
    files::write_json(&folder.join("OriginalSourceManifest.json"), &before)?;
    prepare_descriptor(plan, &snapshot)?;
    prepare_filter(&snapshot)?;
    let paths = files::all_files(&snapshot)?;
    audit(&snapshot, &plan.name, &paths)?;
    let source_manifest = files::manifest(&snapshot, &paths)?;
    files::write_json(&folder.join("SourceManifest.json"), &source_manifest)?;
    let built = folder.join("UAT");
    let args = if build_enabled {
        let arguments = build(
            plan,
            &snapshot,
            &built,
            &folder.join("BuildPlugin.log"),
            reporter,
            config,
        )?;
        reporter.status("正在核对 UAT 输出文件与 SHA256");
        verify_uat(
            &snapshot,
            &built,
            &paths,
            Path::new(plan.descriptor.file_name().context("No descriptor name")?),
        )?;
        Some(arguments)
    } else {
        None
    };
    check_force_stop(reporter)?;
    // UAT must not silently modify the source snapshot used for the submission.
    if files::all_files(&snapshot)? != paths
        || serde_json::to_value(&source_manifest)?
            != serde_json::to_value(files::manifest(&snapshot, &paths)?)?
    {
        bail!("UAT changed the input snapshot");
    }
    let state = if build_enabled {
        "Fab-Source"
    } else {
        "Source-UNVALIDATED"
    };
    let source_zip = folder.join(format!(
        "{}-UE{}-{state}.zip",
        plan.name,
        plan.engine.label()
    ));
    reporter.status("正在压缩源码 ZIP 并回读校验");
    files::archive(&snapshot, &paths, &source_zip)?;
    check_force_stop(reporter)?;
    let binary_zip = if build_enabled && config.binary_archive {
        let path = folder.join(format!(
            "{}-UE{}-{}-Binary.zip",
            plan.name,
            plan.engine.label(),
            plan.plugin.platforms.join("+")
        ));
        reporter.status("正在压缩二进制 ZIP 并回读校验");
        files::archive(&built, &files::all_files(&built)?, &path)?;
        Some(path)
    } else {
        None
    };
    check_force_stop(reporter)?;
    Ok(json!({
        "plugin": plan.name, "plugin_version": files::read_json(&snapshot.join(plan.descriptor.file_name().context("No descriptor name")?))?["VersionName"],
        "source": plan.descriptor, "engine": plan.engine, "target_platforms": plan.plugin.platforms,
        "build_succeeded": build_enabled, "runtime_tests_run": false,
        "low_memory_build": config.low_memory_build && build_enabled,
        "max_parallel_actions": if config.low_memory_build && build_enabled { Some(config.max_parallel_actions) } else { None },
        "uat_arguments": args, "source_archive": source_zip, "binary_archive": binary_zip,
        "source_file_count": paths.len(), "uploaded": false,
        "note": "Technical package only; manual acceptance and marketplace review are separate."
    }))
}

#[derive(Serialize)]
struct Outcome {
    plugin: String,
    engine: Engine,
    directory: PathBuf,
    succeeded: bool,
    error: Option<String>,
}

pub fn release(config: &config::Config, dry_run: bool, source_only: bool) -> Result<()> {
    release_with(config, dry_run, source_only, &Reporter::default())
}

pub fn release_with(
    config: &config::Config,
    dry_run: bool,
    source_only: bool,
    reporter: &Reporter,
) -> Result<()> {
    reporter.status("正在预检查插件、源码与引擎安装");
    let plans = plans(config)?;
    reporter.progress(0, plans.len(), "预检查完成");
    for plan in &plans {
        reporter.log(format!(
            "{} -> UE {} at {} / {} / {} files",
            plan.descriptor.display(),
            plan.engine.label(),
            plan.engine.root.display(),
            plan.plugin.platforms.join("+"),
            plan.paths.len()
        ));
    }
    if dry_run {
        reporter.progress(plans.len(), plans.len(), "预检查通过");
        reporter.status("预检查通过；未创建文件，未启动 Unreal 构建进程。");
        return Ok(());
    }
    let output = files::future_path(&config.output)?;
    fs::create_dir_all(&output)?;
    let run = output.join(format!(
        "release-{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        std::process::id()
    ));
    fs::create_dir(&run)?;
    reporter.output(run.clone());
    let mut outcomes = Vec::new();
    for (index, plan) in plans.iter().enumerate() {
        if reporter.stopping() {
            break;
        }
        reporter.progress(
            index,
            plans.len(),
            format!("{} / UE {}", plan.name, plan.engine.label()),
        );
        let directory = run.join(format!(
            "{:03}-{}-UE{}",
            index + 1,
            plan.name,
            plan.engine.label()
        ));
        fs::create_dir(&directory)?;
        let result = one(plan, &directory, config, source_only, reporter);
        let error = match result {
            Ok(report) => {
                files::write_json(&directory.join("PackagingReport.json"), &report)?;
                reporter.log(format!("打包完成：{}", directory.display()));
                None
            }
            Err(error) => {
                let message = format!("{error:#}");
                reporter.log(format!(
                    "FAILED: {} / UE {}: {message}",
                    plan.name,
                    plan.engine.label()
                ));
                files::write_json(
                    &directory.join("Failure.json"),
                    &json!({"error": message, "uploaded": false}),
                )?;
                Some(message)
            }
        };
        outcomes.push(Outcome {
            plugin: plan.name.clone(),
            engine: plan.engine.clone(),
            directory,
            succeeded: error.is_none(),
            error,
        });
        reporter.progress(
            index + 1,
            plans.len(),
            format!("{} / UE {}", plan.name, plan.engine.label()),
        );
    }
    files::write_json(&run.join("ReleaseReport.json"), &outcomes)?;
    reporter.log(format!(
        "Release report: {}",
        run.join("ReleaseReport.json").display()
    ));
    if reporter.force_stopping() {
        files::write_json(
            &run.join("Stopped.json"),
            &json!({
                "force_terminated": true, "finished_jobs": outcomes.len(), "planned": plans.len(),
                "note": "Unfinished artifacts must not be submitted. Completed jobs and logs are preserved."
            }),
        )?;
        bail!("任务已强制终止；请勿提交本次未完成产物，日志与已完成任务已保留。");
    }
    if outcomes.len() < plans.len() {
        files::write_json(
            &run.join("Stopped.json"),
            &json!({
                "completed": outcomes.len(), "planned": plans.len(),
                "skipped": plans.iter().skip(outcomes.len()).map(|p| json!({"plugin": p.name, "engine": p.engine})).collect::<Vec<_>>()
            }),
        )?;
        bail!(
            "已停止后续任务，已完成 {} / {}；产物和报告已保留",
            outcomes.len(),
            plans.len()
        );
    }
    let failed = outcomes.iter().filter(|o| !o.succeeded).count();
    if failed != 0 {
        bail!(
            "{failed} of {} release jobs failed; successful jobs and logs are preserved",
            outcomes.len()
        );
    }
    Ok(())
}
