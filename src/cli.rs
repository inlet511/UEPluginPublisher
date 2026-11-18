// Copyright 2026 Ken An. All Rights Reserved.

use ue_plugin_publisher::{config, engines, files, package, progress::Reporter, scanner};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    version,
    about = "Universal UE5 plugin release packager. No upload or editor launch."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Inspect a submission ZIP or plugin directory without building or modifying it.
    Inspect {
        path: PathBuf,
        #[arg(long)]
        publisher: Option<String>,
        #[arg(long)]
        year: Option<u32>,
        #[arg(long)]
        engine: Option<String>,
        #[arg(long, value_delimiter = '+')]
        platforms: Vec<String>,
        /// Optional new JSON report path; existing reports are preserved.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// List UE5 installations from Epic Launcher, registry, environment and search roots.
    Engines {
        #[arg(long)]
        search: Vec<PathBuf>,
    },
    /// Create a reusable profile interactively and optionally package it immediately.
    Wizard {
        #[arg(long)]
        profile: Option<PathBuf>,
    },
    /// Package every plugin/engine pair from a saved profile.
    Release {
        #[arg(long)]
        config: PathBuf,
        /// Inspect paths, descriptors and copyright headers; no writes and no UAT.
        #[arg(long)]
        dry_run: bool,
        /// Create clearly marked unvalidated source ZIPs without running UAT.
        #[arg(long)]
        source_only: bool,
    },
}

fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    match default {
        Some(default) => print!("{label} [{default}]: "),
        None => print!("{label}: "),
    }
    io::stdout().flush()?;
    let mut input = String::new();
    if io::stdin().read_line(&mut input)? == 0 {
        bail!("Input closed; no release started");
    }
    let input = input.trim().trim_matches('"');
    Ok(if input.is_empty() {
        default.unwrap_or("").to_string()
    } else {
        input.to_string()
    })
}

fn confirm(label: &str, default: bool) -> Result<bool> {
    loop {
        let answer = prompt(label, Some(if default { "y" } else { "n" }))?;
        match answer.to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("Please enter y or n / 请输入 y 或 n。"),
        }
    }
}

fn default_profile() -> Result<PathBuf> {
    Ok(std::env::current_exe()?
        .parent()
        .context("Executable has no directory")?
        .join("publisher.json"))
}

fn wizard(profile: Option<PathBuf>) -> Result<()> {
    let profile = profile.unwrap_or(default_profile()?);
    println!(
        "UE5 插件发布工具 / UE5 Plugin Publisher\n配置保存后，可一键处理多个插件和多个引擎版本。\n"
    );
    if profile.exists() {
        println!("使用已有配置 / Existing profile: {}", profile.display());
        let config = config::load(&profile)?;
        package::release(&config, true, false)?;
        if confirm(
            "开始发布打包？build=true 时将运行 RunUAT 编译 / Start packaging?",
            true,
        )? {
            return package::release(&config, false, false);
        }
        return Ok(());
    }
    let mut found = engines::discover(&[]);
    for (index, engine) in found.iter().enumerate() {
        println!(
            "  {}. UE {} — {}",
            index + 1,
            engine.label(),
            engine.root.display()
        );
    }
    let mut selected = Vec::new();
    if !found.is_empty() {
        loop {
            let choice = prompt(
                "选择引擎编号，逗号分隔；all=全部；manual=手动 / Engines",
                Some("all"),
            )?;
            if choice.eq_ignore_ascii_case("manual") {
                break;
            }
            if choice.eq_ignore_ascii_case("all") {
                selected.extend(found.drain(..).map(|e| e.root));
                break;
            }
            let indexes: Result<Vec<usize>, _> = choice
                .split(',')
                .map(|x| x.trim().parse::<usize>())
                .collect();
            match indexes {
                Ok(indexes)
                    if !indexes.is_empty()
                        && indexes.iter().all(|i| *i > 0 && *i <= found.len()) =>
                {
                    for index in indexes {
                        selected.push(found[index - 1].root.clone());
                    }
                    break;
                }
                _ => println!("无效选择 / Invalid selection"),
            }
        }
    }
    loop {
        if !selected.is_empty()
            && !confirm(
                "还要手动添加引擎路径？/ Add another engine manually?",
                false,
            )?
        {
            break;
        }
        let path = prompt(
            "UE5 安装目录、Engine 目录或 RunUAT 路径 / Engine path",
            None,
        )?;
        match engines::Engine::read(Path::new(&path)) {
            Ok(engine) => {
                let root = files::canonical(&engine.root)?;
                if !selected.contains(&root) {
                    selected.push(root);
                }
            }
            Err(error) => println!("{error:#}"),
        }
    }
    let mut plugins = Vec::new();
    loop {
        let input = prompt("插件目录或 .uplugin 路径 / Plugin path", None)?;
        let descriptor = match files::descriptor(Path::new(&input)) {
            Ok(path) => path,
            Err(error) => {
                println!("{error:#}");
                continue;
            }
        };
        let default = if cfg!(windows) {
            "Win64"
        } else if cfg!(target_os = "macos") {
            "Mac"
        } else {
            "Linux"
        };
        let platforms = prompt("目标平台，用 + 分隔 / Target platforms", Some(default))?
            .split('+')
            .map(|s| s.trim().to_string())
            .collect();
        plugins.push(config::Plugin {
            path: descriptor,
            platforms,
            engines: Vec::new(),
            exclude: Vec::new(),
        });
        if !confirm("添加另一个插件？/ Add another plugin?", false)? {
            break;
        }
    }
    let output = files::future_path(Path::new(&prompt(
        "发布输出目录 / Output directory",
        Some("releases"),
    )?))?;
    let build = confirm(
        "通过每个 UE5 版本运行 BuildPlugin 编译验证？/ Build each engine version?",
        true,
    )?;
    let binary_archive = build
        && confirm(
            "同时生成本机安装用二进制 ZIP？/ Also create binary ZIP?",
            true,
        )?;
    let config = config::Config {
        output,
        engines: selected,
        plugins,
        build,
        binary_archive,
        low_memory_build: false,
        max_parallel_actions: config::default_parallel_actions(),
    };
    // Load back through the same validation path used for future unattended runs.
    config::write_new(&profile, &config)?;
    println!("配置已保存 / Profile saved: {}", profile.display());
    let config = config::load(&profile)?;
    package::release(&config, true, false)?;
    if confirm("立即开始打包？/ Package now?", true)? {
        package::release(&config, false, false)?;
    }
    Ok(())
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Some(Action::Inspect {
            path,
            publisher,
            year,
            engine,
            platforms,
            report,
        }) => {
            let options = scanner::Options {
                publisher: publisher.unwrap_or_default(),
                year,
                engine: engine.unwrap_or_default(),
                platforms,
            };
            let result = scanner::scan(&path, &options, &Reporter::default())?;
            for finding in &result.findings {
                println!(
                    "[{}] {} {}: {}",
                    finding.severity.label(),
                    finding.rule,
                    finding.path,
                    finding.detail
                );
            }
            println!("{}", result.overall);
            if let Some(path) = report {
                files::write_json(&path, &result)?;
            }
            if !result.scan_complete || result.count(scanner::Severity::Error) > 0 {
                bail!("发现错误或未完成完整扫描");
            }
            Ok(())
        }
        Some(Action::Engines { search }) => {
            let engines = engines::discover(&search);
            if engines.is_empty() {
                println!("未发现 UE5；可通过 wizard 或配置文件手动指定安装路径。");
            }
            for engine in engines {
                println!("UE {}\t{}", engine.label(), engine.root.display());
            }
            Ok(())
        }
        Some(Action::Wizard { profile }) => wizard(profile),
        Some(Action::Release {
            config,
            dry_run,
            source_only,
        }) => package::release(&config::load(&config)?, dry_run, source_only),
        None => wizard(None),
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("ERROR: {error:#}");
        std::process::exit(1);
    }
}
