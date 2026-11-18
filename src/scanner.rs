// Copyright 2026 Ken An. All Rights Reserved.

//! Read-only checks for UE code plugin submissions, never content-project/UEFN rules.
use crate::{files, progress::Reporter};
use anyhow::{bail, Context, Result};
use globset::{GlobBuilder, GlobMatcher};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;
use zip::{CompressionMethod, ZipArchive};

pub const RULESET: &str = "UE-CodePlugin-2026-11-18";
pub const TECHNICAL: &str = "https://www.fab.com/o/technical-requirements";
pub const STRUCTURE: &str = "https://dev.epicgames.com/documentation/en-us/fab/asset-file-format-and-structure-requirements-in-fab";
const TRC: &str = "Volume Heatmap - UE Full TRC.pdf · Code Plugins";
const MAX_FILES: usize = 100_000;
const MAX_TEXT: u64 = 8 * 1024 * 1024;
const MAX_TEXT_TOTAL: u64 = 128 * 1024 * 1024;
const MAX_UNCOMPRESSED: u64 = 32 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Manual,
    Pass,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "错误",
            Self::Warning => "提醒",
            Self::Manual => "待验证",
            Self::Pass => "通过",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Options {
    pub publisher: String,
    pub year: Option<u32>,
    pub engine: String,
    pub platforms: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub rule: String,
    pub path: String,
    pub detail: String,
    pub suggestion: String,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub ruleset: String,
    pub rules_checked_on: String,
    pub official_page_updated: String,
    pub sources: Vec<String>,
    pub scanned_at_unix: u64,
    pub input: PathBuf,
    pub options: Options,
    pub plugin_name: Option<String>,
    pub file_count: usize,
    pub uncompressed_bytes: u64,
    pub inventory: Vec<InventoryFile>,
    pub findings: Vec<Finding>,
    pub scan_complete: bool,
    pub build_run: bool,
    pub overall: String,
}

impl Report {
    fn new(input: &Path, options: &Options) -> Self {
        Self {
            ruleset: RULESET.into(),
            rules_checked_on: "2026-11-18".into(),
            official_page_updated: "2026-09-21".into(),
            sources: vec![TECHNICAL.into(), STRUCTURE.into(), TRC.into()],
            scanned_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            input: input.to_path_buf(),
            options: options.clone(),
            plugin_name: None,
            file_count: 0,
            uncompressed_bytes: 0,
            inventory: Vec::new(),
            findings: Vec::new(),
            scan_complete: true,
            build_run: false,
            overall: String::new(),
        }
    }
    fn add(
        &mut self,
        severity: Severity,
        rule: &str,
        path: &str,
        detail: impl Into<String>,
        suggestion: &str,
        basis: &str,
    ) {
        self.findings.push(Finding {
            severity,
            rule: rule.into(),
            path: path.into(),
            detail: detail.into(),
            suggestion: suggestion.into(),
            basis: basis.into(),
        });
    }
    pub fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == severity)
            .count()
    }
    pub fn markdown(&self) -> String {
        let mut out = format!("# UE 插件上传包静态检查\n\n输入：`{}`\n\n规则集：{}；核对日期：{}；官方技术要求更新日期：{}。\n\n{}\n\n错误 {} · 提醒 {} · 待验证 {} · 通过 {}\n\n",
            self.input.display(), self.ruleset, self.rules_checked_on, self.official_page_updated, self.overall,
            self.count(Severity::Error), self.count(Severity::Warning), self.count(Severity::Manual), self.count(Severity::Pass));
        for f in &self.findings {
            out.push_str(&format!(
                "## [{}] {}\n\n文件：`{}`\n\n{}\n\n建议：{}\n\n依据：{}\n\n",
                f.severity.label(),
                f.rule,
                f.path.replace('`', "'"),
                f.detail.replace('`', "'"),
                f.suggestion,
                f.basis
            ));
        }
        out.push_str("## 规则来源\n\n");
        for source in &self.sources {
            out.push_str(&format!("- {source}\n"));
        }
        out
    }
}

struct Entry {
    name: String,
    bytes: u64,
    directory: bool,
    index: Option<usize>,
}
enum Input {
    Directory(PathBuf),
    Zip(ZipArchive<File>),
}

fn safe_name(raw: &str) -> Option<String> {
    let name = raw.replace('\\', "/");
    let name = name.trim_end_matches('/');
    if name.is_empty()
        || name.starts_with('/')
        || name.contains(':')
        || name.chars().any(|c| c.is_control())
        || name
            .split('/')
            .any(|p| p == ".." || p == "." || p.is_empty())
    {
        return None;
    }
    Some(name.into())
}

fn is_code(name: &str) -> bool {
    matches!(
        name.rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "h" | "hpp" | "hh" | "c" | "cpp" | "cc" | "cxx" | "inl" | "cs" | "usf" | "ush" | "py"
    )
}
fn wants_text(name: &str) -> bool {
    is_code(name)
        || name.ends_with(".uplugin")
        || name.ends_with(".ini")
        || name.ends_with(".md")
        || name.ends_with(".txt")
}

/// Only opening comments count; a notice after executable code doesn't qualify.
fn opening_comments(text: &str, python: bool) -> String {
    let mut output = String::new();
    let mut block = false;
    for (i, line) in text.lines().take(64).enumerate() {
        let line = line.trim();
        if line.is_empty() || (i == 0 && python && line.starts_with("#!")) {
            continue;
        }
        if block {
            output.push_str(line);
            output.push('\n');
            if let Some((_, tail)) = line.split_once("*/") {
                block = false;
                if !tail.trim().is_empty() {
                    break;
                }
            }
        } else if line.starts_with("//") || (python && line.starts_with('#')) {
            output.push_str(line);
            output.push('\n');
        } else if line.starts_with("/*") {
            output.push_str(line);
            output.push('\n');
            if let Some((_, tail)) = line.split_once("*/") {
                if !tail.trim().is_empty() {
                    break;
                }
            } else {
                block = true;
            }
        } else {
            break;
        }
    }
    output
}

fn inventory(input: &mut Input, report: &mut Report) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    match input {
        Input::Directory(root) => {
            for item in WalkDir::new(&*root).follow_links(false) {
                let item = match item {
                    Ok(item) => item,
                    Err(error) => {
                        report.scan_complete = false;
                        report.add(
                            Severity::Error,
                            "读取目录",
                            "",
                            error.to_string(),
                            "确认目录和文件均可读取后重新扫描。",
                            "扫描完整性",
                        );
                        continue;
                    }
                };
                if item.depth() == 0 {
                    continue;
                }
                let name = files::slash(item.path().strip_prefix(&*root)?)?;
                if files::linked(&fs::symlink_metadata(item.path())?) {
                    report.scan_complete = false;
                    report.add(
                        Severity::Error,
                        "链接文件",
                        &name,
                        "不跟随符号链接或 junction；该内容未扫描。",
                        "用实际要提交的文件代替链接。",
                        "扫描完整性",
                    );
                    continue;
                }
                if safe_name(&name).is_none() {
                    report.add(
                        Severity::Error,
                        "路径结构",
                        &name,
                        "路径包含不可移植或歧义字符。",
                        "使用正常的插件相对路径。",
                        "ZIP / 路径完整性",
                    );
                    continue;
                }
                entries.push(Entry {
                    name,
                    bytes: item.metadata()?.len(),
                    directory: item.file_type().is_dir(),
                    index: None,
                });
                if entries.len() > MAX_FILES {
                    bail!("超过 {MAX_FILES} 个条目的扫描上限；未完成扫描");
                }
            }
        }
        Input::Zip(zip) => {
            if zip.len() > MAX_FILES {
                bail!("ZIP 超过 {MAX_FILES} 个条目，未完成扫描");
            }
            for i in 0..zip.len() {
                let entry = zip.by_index_raw(i)?;
                let raw = entry.name();
                let Some(name) = safe_name(raw) else {
                    report.add(
                        Severity::Error,
                        "ZIP 路径",
                        raw,
                        "ZIP 含绝对路径、上级跳转或歧义路径。",
                        "重新生成只有插件相对路径的 ZIP。",
                        "ZIP / 路径完整性",
                    );
                    continue;
                };
                if entry.encrypted() {
                    report.add(
                        Severity::Error,
                        "ZIP 加密",
                        &name,
                        "ZIP 条目已加密。",
                        "取消密码和加密，重新打包。",
                        "Fab · Unreal Engine Project Format",
                    );
                }
                if !matches!(
                    entry.compression(),
                    CompressionMethod::Stored | CompressionMethod::Deflated
                ) {
                    report.add(
                        Severity::Error,
                        "ZIP 压缩方法",
                        &name,
                        format!("不支持完整读取此压缩方法：{:?}", entry.compression()),
                        "使用常规 ZIP Store 或 Deflate。",
                        "扫描完整性 / Fab 可读取 ZIP",
                    );
                }
                if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                    report.add(
                        Severity::Error,
                        "ZIP 链接",
                        &name,
                        "ZIP 中含符号链接。",
                        "改为包含实际发布文件。",
                        "ZIP / 路径完整性",
                    );
                }
                entries.push(Entry {
                    name,
                    bytes: if entry.is_dir() { 0 } else { entry.size() },
                    directory: entry.is_dir(),
                    index: Some(i),
                });
            }
        }
    }
    let mut names = BTreeSet::new();
    for entry in &entries {
        if !names.insert(entry.name.to_ascii_lowercase()) {
            report.add(
                Severity::Error,
                "重复路径",
                &entry.name,
                "路径重复或仅大小写不同，解压后可能覆盖。",
                "删除重复条目并统一大小写。",
                "ZIP / 路径完整性",
            );
        }
    }
    Ok(entries)
}

fn read_entry(input: &mut Input, entry: &Entry, capture: bool) -> Result<(String, Option<String>)> {
    let mut reader: Box<dyn Read + '_> = match input {
        Input::Directory(root) => Box::new(File::open(root.join(&entry.name))?),
        Input::Zip(zip) => Box::new(zip.by_index(entry.index.context("ZIP index missing")?)?),
    };
    let mut hash = Sha256::new();
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 65536];
    let mut total = 0_u64;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > entry.bytes || (capture && total > MAX_TEXT) {
            bail!("文件尺寸变化或超过扫描容量");
        }
        hash.update(&buffer[..count]);
        if capture {
            bytes.extend_from_slice(&buffer[..count]);
        }
    }
    if total != entry.bytes {
        bail!("读取长度与文件目录记录不符");
    }
    let text = if capture {
        Some(
            String::from_utf8(bytes)
                .context("文本不是可读取的 UTF-8")?
                .trim_start_matches('\u{feff}')
                .to_string(),
        )
    } else {
        None
    };
    Ok((format!("{:x}", hash.finalize()), text))
}

struct Filter {
    rules: Vec<(bool, GlobMatcher)>,
    uncertain: bool,
}
impl Filter {
    fn parse(text: &str) -> Self {
        let mut result = Self {
            rules: Vec::new(),
            uncertain: false,
        };
        let mut active = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                active = line == "[FilterPlugin]";
                continue;
            }
            if !active || line.is_empty() || line.starts_with(';') {
                continue;
            }
            let (include, rule) = if let Some(rule) = line.strip_prefix('-') {
                (false, rule)
            } else {
                (true, line.strip_prefix('+').unwrap_or(line))
            };
            if rule.contains('[') || rule.contains(']') || rule.contains('\\') {
                result.uncertain = true;
                continue;
            }
            let glob = rule.trim_start_matches('/').replace("...", "**");
            match GlobBuilder::new(&glob).literal_separator(false).build() {
                Ok(glob) => result.rules.push((include, glob.compile_matcher())),
                Err(_) => result.uncertain = true,
            }
        }
        result
    }
    fn included(&self, name: &str, default: bool) -> bool {
        self.rules.iter().fold(
            default,
            |value, (include, glob)| if glob.is_match(name) { *include } else { value },
        )
    }
}

fn descriptor_checks(
    report: &mut Report,
    path: &str,
    value: &Value,
    names: &BTreeSet<String>,
    options: &Options,
) {
    if !value.is_object() || value["FileVersion"].as_u64().is_none() {
        report.add(
            Severity::Error,
            "描述符结构",
            path,
            "缺少合法的 FileVersion 或根 JSON 对象。",
            "修正 .uplugin JSON。",
            "Fab §4.3.6",
        );
    }
    for field in ["FriendlyName", "CreatedBy", "VersionName"] {
        if value[field].as_str().unwrap_or("").trim().is_empty() {
            report.add(
                Severity::Warning,
                "描述符信息",
                path,
                format!("{field} 为空或缺失。"),
                "填写真实的插件名称、作者和版本。",
                "发布信息完整性建议",
            );
        }
    }
    let version = value["EngineVersion"].as_str().unwrap_or("");
    let pieces: Vec<_> = version.split('.').collect();
    if pieces.len() != 3
        || pieces[0] != "5"
        || pieces[1].parse::<u32>().is_err()
        || pieces[2] != "0"
    {
        report.add(
            Severity::Error,
            "引擎版本",
            path,
            format!("EngineVersion 应为明确的 5.x.0，实际为 {version:?}。"),
            "为每个支持的小版本生成独立包并填写 EngineVersion。",
            "Fab §4.3.6.a / §4.2.2.d / TRC",
        );
    } else if !options.engine.is_empty() && version != options.engine {
        report.add(
            Severity::Error,
            "目标版本一致性",
            path,
            format!("包声明 {version}，预期 {}。", options.engine),
            "选择对应引擎重新打包。",
            "Fab §4.2.2.d",
        );
    } else {
        report.add(
            Severity::Pass,
            "引擎版本",
            path,
            format!("EngineVersion={version}"),
            "仍需在此版本实际构建。",
            "Fab §4.3.6.a",
        );
    }
    let fab = value["FabURL"].as_str().unwrap_or("");
    if !fab.starts_with("com.epicgames.launcher://ue/Fab/product/")
        || fab.rsplit('/').next().unwrap_or("").is_empty()
    {
        report.add(Severity::Warning, "FabURL", path, "缺少有效的 FabURL；已有商品 ID 时，这是提交前必须补齐的字段。", "使用发布者门户真实产品 ID 填写 com.epicgames.launcher://ue/Fab/product/<ID>，不要编造 ID。", "Fab §4.3.6.c");
    }
    let Some(modules) = value["Modules"].as_array().filter(|m| !m.is_empty()) else {
        report.add(
            Severity::Error,
            "代码模块",
            path,
            "代码插件需要至少一个 C++ 模块。",
            "在 Modules 中声明实际模块并包含 Source。",
            "Fab §4.3.6.1.c",
        );
        return;
    };
    let mut module_names = BTreeSet::new();
    for module in modules {
        let name = module["Name"].as_str().unwrap_or("");
        if name.is_empty()
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || !module_names.insert(name)
            || module["Type"].as_str().unwrap_or("").is_empty()
        {
            report.add(
                Severity::Error,
                "模块描述",
                path,
                format!("模块 Name/Type 无效或模块名重复：{name:?}"),
                "修正模块描述，名称使用英文标识符。",
                "UE 模块描述符",
            );
            continue;
        }
        let build = format!("Source/{name}/{name}.Build.cs");
        if !names.contains(&build) {
            report.add(
                Severity::Error,
                "模块源码",
                &build,
                "没有找到模块对应的 Build.cs。",
                "包含真实模块规则和可读 C++ 源码。",
                "Fab §4.3.6.1.a / .c",
            );
        }
        let allow = module.get("PlatformAllowList");
        let deny = module.get("PlatformDenyList");
        if allow.is_none() && deny.is_none() {
            report.add(
                Severity::Error,
                "模块目标平台",
                path,
                format!("模块 {name} 缺少 PlatformAllowList / PlatformDenyList。"),
                "为每个模块声明目标平台限制。",
                "Fab §4.3.6.b / TRC",
            );
        }
        for (key, list) in [("PlatformAllowList", allow), ("PlatformDenyList", deny)] {
            if let Some(list) = list {
                if !list
                    .as_array()
                    .is_some_and(|l| l.iter().all(|p| p.as_str().is_some_and(|p| !p.is_empty())))
                {
                    report.add(
                        Severity::Error,
                        "模块目标平台",
                        path,
                        format!("模块 {name} 的 {key} 应为平台字符串数组。"),
                        "修正平台数组。",
                        "Fab §4.3.6.b",
                    );
                }
            }
        }
        if !options.platforms.is_empty() && module["Type"].as_str() == Some("Runtime") {
            for platform in &options.platforms {
                let allowed = allow.and_then(Value::as_array).map_or(true, |a| {
                    a.iter().any(|p| p.as_str() == Some(platform.as_str()))
                });
                let denied = deny
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.iter().any(|p| p.as_str() == Some(platform.as_str())));
                if !allowed || denied {
                    report.add(
                        Severity::Warning,
                        "平台范围核对",
                        path,
                        format!("Runtime 模块 {name} 不支持预期平台 {platform}。"),
                        "核对模块设计与商品页支持平台是否一致。",
                        "Fab §4.2.3 / TRC",
                    );
                }
            }
        }
    }
    if let Some(dependencies) = value["Plugins"].as_array() {
        for dep in dependencies
            .iter()
            .filter(|d| d["Enabled"].as_bool() == Some(true))
        {
            report.add(
                Severity::Manual,
                "插件依赖",
                path,
                format!("启用依赖：{}；静态包不能判定是否随引擎提供。", dep["Name"]),
                "确认它是随引擎提供的插件，并在文档/商品页声明。不得依赖其他用户制作的插件。",
                "Fab §4.3.6.d",
            );
        }
    }
}

pub fn scan(path: &Path, options: &Options, reporter: &Reporter) -> Result<Report> {
    if options
        .year
        .is_some_and(|year| !(1900..=2100).contains(&year))
    {
        bail!("预期发行年份应在 1900–2100 之间");
    }
    let path = files::canonical(path)?;
    let path = if path.extension().is_some_and(|e| e == "uplugin") {
        path.parent().context("No plugin directory")?.to_path_buf()
    } else {
        path
    };
    let mut report = Report::new(&path, options);
    reporter.status("正在读取上传包目录；扫描不会解压、修改文件或调用 UE");
    let mut input = if path.is_dir() {
        Input::Directory(path.clone())
    } else {
        Input::Zip(ZipArchive::new(File::open(&path)?).context("请选择插件目录或可读取的 ZIP")?)
    };
    let entries = inventory(&mut input, &mut report)?;
    let file_entries: Vec<_> = entries.iter().filter(|e| !e.directory).collect();
    report.file_count = file_entries.len();
    report.uncompressed_bytes = file_entries.iter().try_fold(0_u64, |total, e| {
        total.checked_add(e.bytes).context("ZIP 尺寸溢出")
    })?;
    if report.uncompressed_bytes > MAX_UNCOMPRESSED {
        bail!("解压后总尺寸超过 32 GiB 扫描上限，未完成扫描");
    }
    if report.uncompressed_bytes > 15_000_000_000 {
        report.add(
            Severity::Warning,
            "包大小",
            "",
            "未压缩内容超过 15 GB。",
            "按官方建议减少体积，必要时先联系 Fab 确认。",
            "Fab · File Format Requirements（建议 ≤15GB）",
        );
    }
    let descriptors: Vec<_> = file_entries
        .iter()
        .filter(|e| e.name.to_ascii_lowercase().ends_with(".uplugin"))
        .collect();
    let projects = file_entries
        .iter()
        .any(|e| e.name.to_ascii_lowercase().ends_with(".uproject"));
    if descriptors.len() != 1 || projects {
        report.add(
            Severity::Error,
            "单插件结构",
            "",
            format!(
                "发现 {} 个 .uplugin；包含 .uproject={projects}。",
                descriptors.len()
            ),
            "一个上传 ZIP 只包含一个插件；示例项目应单独提供。",
            "Fab · ZIP 结构 / §4.3.6.3.c / TRC",
        );
    }
    let descriptor_path = descriptors.first().map(|e| e.name.clone());
    let prefix = descriptor_path
        .as_deref()
        .and_then(|p| p.rfind('/').map(|i| &p[..=i]))
        .unwrap_or("")
        .to_string();
    if prefix.trim_end_matches('/').contains('/') {
        report.add(
            Severity::Error,
            "外层嵌套",
            &prefix,
            "插件描述符嵌套超过一层包装目录。",
            "将 ZIP 根设为插件内容，或只包一层插件目录。",
            "Fab · Unreal Engine Code Plugins / TRC",
        );
    }
    let plugin_name = descriptor_path
        .as_deref()
        .and_then(|p| p.rsplit('/').next())
        .map(|p| p.trim_end_matches(".uplugin").to_string());
    report.plugin_name = plugin_name.clone();
    let container_name = if prefix.is_empty() {
        match &input {
            Input::Directory(root) => root
                .file_name()
                .and_then(|name| name.to_str())
                .map(String::from),
            Input::Zip(_) => plugin_name.clone(),
        }
    } else {
        Some(prefix.trim_end_matches('/').to_string())
    };
    let mut texts = BTreeMap::new();
    let mut text_total = 0_u64;
    for (i, entry) in file_entries.iter().enumerate() {
        if reporter.stopping() {
            bail!("扫描已停止；未生成完整报告");
        }
        if i % 64 == 0 {
            reporter.progress(i, file_entries.len(), "读取文件与校验 ZIP CRC");
        }
        let capture = wants_text(&entry.name)
            && entry.bytes <= MAX_TEXT
            && text_total.saturating_add(entry.bytes) <= MAX_TEXT_TOTAL;
        if capture {
            text_total += entry.bytes;
        } else if wants_text(&entry.name) {
            report.scan_complete = false;
            report.add(
                Severity::Manual,
                "文本容量",
                &entry.name,
                "文本超过单文件 8 MiB 或总计 128 MiB 的读取上限。",
                "人工检查该文件的源码、版权与配置。",
                "扫描完整性",
            );
        }
        let (sha256, text) = match read_entry(&mut input, entry, capture) {
            Ok((hash, text)) => (Some(hash), text),
            Err(error) => {
                report.scan_complete = false;
                report.add(
                    Severity::Error,
                    "文件读取 / ZIP CRC",
                    &entry.name,
                    format!("{error:#}"),
                    "重新生成可完整读取的发布包；源码使用 UTF-8。",
                    "扫描完整性 / Fab 可读取 ZIP",
                );
                (None, None)
            }
        };
        report.inventory.push(InventoryFile {
            path: entry.name.clone(),
            bytes: entry.bytes,
            sha256,
        });
        if let Some(text) = text {
            texts.insert(entry.name.clone(), text);
        }
    }
    let mut names = BTreeSet::new();
    let mut directories = BTreeSet::new();
    for entry in &entries {
        if !prefix.is_empty() && entry.name == prefix.trim_end_matches('/') && entry.directory {
            continue;
        }
        let Some(relative) = entry.name.strip_prefix(&prefix) else {
            report.add(
                Severity::Error,
                "插件外文件",
                &entry.name,
                "文件在插件包装目录之外。",
                "移除插件之外的项目、报告和其他内容。",
                "Fab · ZIP 只包含一个插件",
            );
            continue;
        };
        if entry.directory {
            directories.insert(relative.to_string());
        } else {
            names.insert(relative.to_string());
        }
        if let Some((top, _)) = relative.split_once('/') {
            directories.insert(top.to_string());
        }
        let lower = relative.to_ascii_lowercase();
        let top = lower.split('/').next().unwrap_or("");
        if matches!(
            top,
            "binaries"
                | "build"
                | "intermediate"
                | "saved"
                | "deriveddatacache"
                | "hostproject"
                | ".git"
                | ".vs"
        ) || lower
            .split('/')
            .any(|p| matches!(p, "__pycache__" | "node_modules"))
        {
            report.add(
                Severity::Error,
                "本地构建目录",
                relative,
                "源码上传包包含本地构建目录、缓存或版本控制目录。",
                "清理发布副本；不要上传二进制安装包作为源码包。",
                "Fab §4.3.7.3.a / TRC",
            );
        }
        if container_name
            .as_ref()
            .is_some_and(|n| format!("{n}/{relative}").chars().count() > 170)
        {
            report.add(
                Severity::Error,
                "170 字符路径",
                relative,
                "包含插件顶层目录名后路径超过 170 字符。",
                "缩短文件名与目录层级。",
                "Fab §4.3.7.3.c / TRC",
            );
        }
        if matches!(lower.rsplit('.').next().unwrap_or(""), "exe" | "msi") {
            report.add(
                Severity::Error,
                "禁止分发可执行安装文件",
                relative,
                "代码插件包不得分发 .exe / .msi。",
                "移出插件提交包，发布工具本身应独立保存。",
                "Fab §4.3.6.1.e",
            );
        }
        if lower.ends_with(".py") && !relative.starts_with("Content/Python/") {
            report.add(
                Severity::Error,
                "Python 位置",
                relative,
                "插件 Python 位于 Content/Python 之外。",
                "移动自有代码到 Content/Python；第三方代码放 Content/Python/Lib/site-packages。",
                "Fab · Python Scripts / TRC",
            );
        }
        if relative.split('/').any(|part| part == "ThirdParty")
            && !relative.starts_with("Source/ThirdParty/")
            && !relative.starts_with("Content/Python/")
            && !entry.directory
        {
            report.add(Severity::Warning, "第三方目录位置", relative,
                "命名为 ThirdParty 的内容不在 Source/ThirdParty 内（依据目录名推断）。",
                "确认实际来源；C++ 第三方依赖放 Source/ThirdParty，Python 第三方依赖放 Content/Python/Lib/site-packages。",
                "Fab §4.3.7.3.d / Python Scripts / TRC");
        }
        // English identifier constraints apply to asset/package names. Documentation
        // filenames such as README.zh-CN.md are checked manually, not rejected.
        if relative.starts_with("Content/")
            && (lower.ends_with(".uasset") || lower.ends_with(".umap"))
        {
            let stem = relative.rsplit_once('.').map(|x| x.0).unwrap_or(relative);
            if stem
                .chars()
                .any(|c| c != '/' && c != '_' && !c.is_ascii_alphanumeric())
            {
                report.add(
                    Severity::Error,
                    "UE 资产命名",
                    relative,
                    "资产或目录名包含英文数字、下划线之外的字符。",
                    "在编辑器内重命名并修复重定向器。",
                    "Fab §4.3.7.1.c",
                );
            }
        }
    }
    for required in ["Source", "Config", "Content"] {
        if !directories.contains(required) {
            report.add(
                Severity::Warning,
                "代码插件标准结构",
                required,
                "没有找到官方列出的标准目录。纯代码插件可能没有内容资产。",
                "核对官方标准结构及本产品需求；不要为凑目录添加无用文件。",
                "Fab · Unreal Engine Code Plugins",
            );
        }
    }
    let get_text = |name: &str| texts.get(&format!("{prefix}{name}"));
    if let Some(descriptor_path) = &descriptor_path {
        match texts
            .get(descriptor_path)
            .map(|text| serde_json::from_str::<Value>(text))
        {
            Some(Ok(value)) => {
                descriptor_checks(&mut report, descriptor_path, &value, &names, options)
            }
            _ => report.add(
                Severity::Error,
                "描述符 JSON",
                descriptor_path,
                "无法读取或解析插件描述符 JSON。",
                "修正 UTF-8 JSON 格式。",
                "UE .uplugin 格式",
            ),
        }
    }
    let mut copyright_checked = 0;
    for name in &names {
        let third_party = name.split('/').any(|p| p == "ThirdParty")
            || name.starts_with("Content/Python/Lib/site-packages/");
        if is_code(name) && !third_party {
            if let Some(text) = get_text(name) {
                copyright_checked += 1;
                let opening = opening_comments(text, name.ends_with(".py"));
                let notices: Vec<_> = opening
                    .lines()
                    .filter(|l| l.to_lowercase().contains("copyright"))
                    .collect();
                let valid = notices.iter().any(|line| {
                    let lower = line.to_lowercase();
                    let year = line.split(|c: char| !c.is_ascii_digit()).any(|p| {
                        p.len() == 4
                            && p.parse::<u32>().is_ok_and(|y| {
                                (1900..=2100).contains(&y)
                                    && options.year.map_or(true, |expected| expected == y)
                            })
                    });
                    let owner = if options.publisher.trim().is_empty() {
                        let words: Vec<_> = lower
                            .split(|c: char| !c.is_alphabetic())
                            .filter(|w| !w.is_empty())
                            .collect();
                        words
                            .iter()
                            .any(|w| !["copyright", "c", "all", "rights", "reserved"].contains(w))
                    } else {
                        lower.contains(&options.publisher.to_lowercase())
                    };
                    year && owner && !lower.contains("epic games")
                });
                if !valid {
                    report.add(
                        Severity::Error,
                        "版权头",
                        name,
                        "文件开头没有包含发行者和发行年份的有效版权注释，或仅有 Epic 模板声明。",
                        "在文件顶部添加真实发行者和计划发行年份；可在检查设置中指定精确匹配。",
                        "Fab §4.3.6.1.b / TRC",
                    );
                }
                if name.ends_with(".Build.cs") && !text.contains("UseExplicitOrSharedPCHs") {
                    report.add(
                        Severity::Manual,
                        "IWYU",
                        name,
                        "未找到 UseExplicitOrSharedPCHs；可能使用其他合法配置。",
                        "检查模块的 IWYU 设置并在对应 UE 版本编译验证。",
                        "Fab §4.3.6.2.c",
                    );
                }
                if name.ends_with(".py")
                    && (text.contains("C:\\")
                        || text.contains("C:/")
                        || text.contains("/Users/")
                        || text.contains("/home/"))
                {
                    report.add(
                        Severity::Warning,
                        "Python 本地路径",
                        name,
                        "发现疑似本机绝对路径（启发式判断，可能位于示例注释中）。",
                        "检查路径是否妨碍其他用户使用，改用相对路径或配置。",
                        "Fab · Python Scripts",
                    );
                }
            }
        }
        if matches!(
            name.rsplit('.').next().unwrap_or(""),
            "dll" | "lib" | "a" | "so" | "dylib"
        ) {
            let proper = name.starts_with("Source/ThirdParty/");
            report.add(if proper { Severity::Manual } else { Severity::Warning }, "第三方编译库", name,
                "发现编译库，无法仅凭文件判断来源或是否依赖 UE 源码。", "仅允许不依赖 UE 源码的外部库；确认授权，放 Source/ThirdParty，提交第三方软件声明。", "Fab §4.3.6.1.a / §4.3.7.3.d");
        }
    }
    if !names
        .iter()
        .any(|n| n.starts_with("Source/") && n.ends_with(".cpp"))
    {
        report.add(
            Severity::Error,
            "可读 C++ 源码",
            "Source",
            "没有找到 C++ .cpp 文件。",
            "代码插件必须包含实际模块及依赖 UE 的可读源代码。",
            "Fab §4.3.6.1.a / .c",
        );
    }
    if !report.findings.iter().any(|f| f.rule == "版权头") && copyright_checked > 0 {
        report.add(
            Severity::Pass,
            "版权头",
            "",
            format!("已检查 {copyright_checked} 个自有源码文件的开头。"),
            "确保版权归属和年份准确。",
            "Fab §4.3.6.1.b / TRC",
        );
    }
    let filter = get_text("Config/FilterPlugin.ini").map(|t| Filter::parse(t));
    for name in names
        .iter()
        .filter(|name| !name.contains('/') && !name.to_ascii_lowercase().ends_with(".uplugin"))
    {
        if !filter.as_ref().is_some_and(|f| f.included(name, false)) {
            report.add(
                Severity::Error,
                "FilterPlugin 根文件",
                name,
                "插件根目录的额外文件没有显式打包规则，可能被 UAT 丢弃。",
                "在 [FilterPlugin] 中列出该文件（如 /README.md），或从上传包移除无用文件。",
                "TRC · FilterPlugin 包含额外文件 / UAT 文件过滤",
            );
        }
    }
    let custom: BTreeSet<_> = names
        .iter()
        .filter_map(|n| n.split_once('/').map(|(top, _)| top.to_string()))
        .filter(|top| !["Content", "Resources", "Source"].contains(&top.as_str()))
        .collect();
    for top in custom {
        let members: Vec<_> = names
            .iter()
            .filter(|n| n.starts_with(&format!("{top}/")))
            .collect();
        let missing: Vec<_> = members
            .iter()
            .filter(|n| !filter.as_ref().is_some_and(|f| f.included(n, false)))
            .collect();
        if !missing.is_empty() {
            report.add(Severity::Error, "FilterPlugin 自定义目录", &top, format!("{} 个文件未被基础 FilterPlugin.ini 包含，例如 {}。", missing.len(), missing[0]), "在 Config/FilterPlugin.ini 的 [FilterPlugin] 中包含该目录，例如 /Shaders/...；需要排除的内容应从上传包移除。", "Fab §4.3.7.3.b / TRC");
        }
    }
    if filter.as_ref().is_some_and(|f| f.uncertain)
        || names
            .iter()
            .any(|n| n.starts_with("Config/FilterPlugin") && n != "Config/FilterPlugin.ini")
    {
        report.add(
            Severity::Manual,
            "完整 UAT 过滤规则",
            "Config",
            "存在平台专用或复杂过滤规则；静态匹配只覆盖常见 glob 语法。",
            "以每个版本/平台的 UAT 输出完整性校验为准。",
            "UE UAT FileFilter / TRC",
        );
    }
    let docs = names
        .iter()
        .any(|n| n.starts_with("Docs/") || n.to_ascii_lowercase().starts_with("readme"));
    if !docs {
        report.add(
            Severity::Warning,
            "文档入口",
            "",
            "未找到 Docs 或 README；文档可能通过外链、视频或编辑器提供。",
            "提供英文安装与使用文档，并人工核对内容完整性。",
            "Fab §4.3.8 / TRC",
        );
    }
    for (rule, detail, basis) in [
        (
            "单插件结构",
            "只包含一个插件描述符，没有混入示例项目。",
            "Fab · ZIP 结构 / TRC",
        ),
        (
            "170 字符路径",
            "已检查文件路径（包含插件顶层目录名）。",
            "Fab §4.3.7.3.c / TRC",
        ),
        (
            "本地构建目录",
            "未发现检查范围内的本地构建目录和缓存。",
            "Fab §4.3.7.3.a / TRC",
        ),
        (
            "禁止分发可执行安装文件",
            "没有发现 .exe / .msi。",
            "Fab §4.3.6.1.e",
        ),
        (
            "Python 位置",
            "已检查 Python 文件路径；无 Python 的插件同样适用。",
            "Fab · Python Scripts / TRC",
        ),
        (
            "FilterPlugin 自定义目录",
            "基础过滤规则包含检查到的自定义目录文件。",
            "Fab §4.3.7.3.b / TRC",
        ),
    ] {
        if report.scan_complete && !report.findings.iter().any(|f| f.rule == rule) {
            report.add(
                Severity::Pass,
                rule,
                "",
                detail,
                "仍需完成报告中的人工与构建验证。",
                basis,
            );
        }
    }
    for (rule, detail, suggestion, basis) in [
        ("实际构建与运行", "本扫描没有运行 UAT、编译器、编辑器或游戏。", "在每个声明的 UE 版本和平台构建、安装并人工验收；核对错误及相关警告。", "Fab §4.3.6.2 / TRC"),
        ("商品页与依赖说明", "未访问商品页，无法验证英文描述、媒体、技术信息、支持平台及外链。", "人工核对商品页与实际包一致，首次提交和版本支持政策按当前官方要求确认。", "Fab §4.1 / §4.2 / TRC"),
        ("第三方许可与来源", "扫描无法证明源码、字体、模型或其他资产的授权归属。", "核对所有第三方来源、许可、Source/ThirdParty 与 Python site-packages 位置，并完成官方声明。", "Fab §4.2.5 / §4.3.7.3.d / Python Scripts / TRC"),
        ("资产引用与英文文档", "仅检查文件层面，未解析 UE 二进制资产、重定向器、地图光照或文档语言质量。", "在 UE 中清理重定向器并核对依赖、示例地图和完整英文文档。", "Fab §4.3.1.h / §4.3.2 / §4.3.8 / TRC"),
        ("文件命名与内容相关性", "仅对 UE 资产包名执行严格自动命名检查；不能判断所有目录的语义或文件是否有用。", "核对全部文件/目录采用英文且命名一致，不含无关、未使用或内部草稿内容。", "Fab §4.3.7.1 / §4.3.7.3.a"),
    ] { report.add(Severity::Manual, rule, "", detail, suggestion, basis); }
    if report.count(Severity::Error) == 0 && report.scan_complete {
        report.overall =
            "未发现已实现静态规则中的错误；仍有待人工/构建验证项目，不代表 Fab 审核通过。".into();
    } else {
        report.overall = "发现提交问题或未完整扫描的内容；请处理错误并核对待验证项目。".into();
    }
    reporter.progress(file_entries.len(), file_entries.len(), "扫描完成");
    reporter.status(format!(
        "扫描完成：{} 个错误，{} 个提醒，{} 个待验证项",
        report.count(Severity::Error),
        report.count(Severity::Warning),
        report.count(Severity::Manual)
    ));
    Ok(report)
}
