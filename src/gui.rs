// Copyright 2026 Ken An. All Rights Reserved.

use anyhow::{bail, Context, Result};
use eframe::egui::{self, Color32, RichText};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::Ordering,
    mpsc::{self, Receiver, SyncSender},
};
use ue_plugin_publisher::{
    config,
    engines::{self, Engine},
    files, package,
    progress::{Event, Reporter},
    scanner::{self, Options, Report, Severity},
};

const BACKGROUND: Color32 = Color32::from_rgb(21, 24, 29);
const SURFACE: Color32 = Color32::from_rgb(25, 30, 36);
const CARD: Color32 = Color32::from_rgb(33, 38, 44);
const BORDER: Color32 = Color32::from_rgb(47, 55, 62);
const TEXT: Color32 = Color32::from_rgb(225, 230, 234);
const MUTED: Color32 = Color32::from_rgb(176, 187, 196);
const ACCENT: Color32 = Color32::from_rgb(103, 187, 207);
const PRIMARY: Color32 = Color32::from_rgb(21, 83, 101);
const TAB_SELECTED: Color32 = Color32::from_rgb(27, 152, 180);
const TAB_TEXT: Color32 = Color32::from_rgb(12, 33, 42);
const SELECTED: Color32 = Color32::from_rgb(24, 77, 94);
const SELECTED_TITLE: Color32 = Color32::from_rgb(30, 48, 57);
const BUTTON: Color32 = Color32::from_rgb(49, 53, 59);
const BUTTON_HOVER: Color32 = Color32::from_rgb(61, 69, 77);
const INPUT: Color32 = Color32::from_rgb(17, 20, 24);
const INPUT_BORDER: Color32 = Color32::from_rgb(45, 91, 104);
const CHECK_MARK: Color32 = Color32::from_rgb(164, 207, 218);
const ICON: Color32 = Color32::from_rgb(165, 181, 192);
const ERROR: Color32 = Color32::from_rgb(199, 139, 139);
const WARN: Color32 = Color32::from_rgb(189, 166, 123);

fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .inner_margin(16)
        .stroke(egui::Stroke::new(1.0_f32, BORDER))
}

fn card(ui: &mut egui::Ui, title: &str, subtitle: &str, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(CARD)
        .inner_margin(16)
        .corner_radius(8)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(title).size(18.0));
            if !subtitle.is_empty() {
                ui.label(RichText::new(subtitle).small().color(MUTED));
            }
            ui.add_space(5.0);
            contents(ui);
        });
    ui.add_space(6.0);
}

fn primary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(format!("</>  {text}")).color(TEXT))
        .fill(PRIMARY)
        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(35, 99, 117)))
        .min_size(egui::vec2(140.0, 26.0))
}

#[derive(Clone, Copy)]
enum ActionIcon {
    Folder,
    Save,
    Edit,
}

// Draw small native vector icons, without depending on symbol-font coverage.
fn action_button(ui: &mut egui::Ui, icon: ActionIcon, text: &str) -> egui::Response {
    let response = ui.button(format!("    {text}"));
    let origin = egui::pos2(response.rect.left() + 6.0, response.rect.center().y - 6.0);
    let stroke = egui::Stroke::new(1.0_f32, if response.enabled() { TEXT } else { MUTED });
    let points: &[[f32; 2]] = match icon {
        ActionIcon::Folder => &[
            [0.0, 11.0],
            [0.0, 1.0],
            [4.0, 1.0],
            [6.0, 3.0],
            [12.0, 3.0],
            [12.0, 11.0],
            [0.0, 11.0],
        ],
        ActionIcon::Save => &[
            [0.0, 0.0],
            [10.0, 0.0],
            [12.0, 2.0],
            [12.0, 12.0],
            [0.0, 12.0],
            [0.0, 0.0],
            [3.0, 0.0],
            [3.0, 4.0],
            [9.0, 4.0],
            [9.0, 0.0],
        ],
        ActionIcon::Edit => &[
            [2.0, 9.0],
            [3.0, 6.0],
            [9.0, 0.0],
            [12.0, 3.0],
            [6.0, 9.0],
            [2.0, 9.0],
        ],
    };
    ui.painter().add(egui::Shape::line(
        points
            .iter()
            .map(|p| origin + egui::vec2(p[0], p[1]))
            .collect(),
        stroke,
    ));
    if matches!(icon, ActionIcon::Save) {
        ui.painter().add(egui::Shape::line(
            vec![
                origin + egui::vec2(3.0, 12.0),
                origin + egui::vec2(3.0, 7.0),
                origin + egui::vec2(9.0, 7.0),
                origin + egui::vec2(9.0, 12.0),
            ],
            stroke,
        ));
    }
    response
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Publish,
    Inspect,
}
#[derive(Clone, Copy, PartialEq)]
enum Work {
    Discover,
    Preflight,
    Publish,
    Inspect,
}
enum Product {
    Engines(Vec<Engine>),
    Published,
    Inspected(Box<Report>),
}
enum Message {
    Event(Event),
    Finished(std::result::Result<Product, String>),
}

struct EngineRow {
    root: PathBuf,
    label: String,
    selected: bool,
}
struct PluginRow {
    path: String,
    platforms: String,
    exclude: String,
    custom_engines: bool,
    engines: Vec<PathBuf>,
}
impl From<config::Plugin> for PluginRow {
    fn from(p: config::Plugin) -> Self {
        Self {
            path: p.path.display().to_string(),
            platforms: p.platforms.join("+"),
            exclude: p.exclude.join("\n"),
            custom_engines: !p.engines.is_empty(),
            engines: p.engines,
        }
    }
}

fn key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    let value = value.strip_prefix("//?/").unwrap_or(&value).to_string();
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}
fn same(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}
fn list(text: &str) -> Vec<String> {
    text.split(['+', ',', ';', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}
fn color(severity: Severity) -> Color32 {
    match severity {
        Severity::Error => ERROR,
        Severity::Warning => WARN,
        Severity::Manual => MUTED,
        Severity::Pass => ACCENT,
    }
}
fn input_colors(ui: &mut egui::Ui) {
    ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, INPUT_BORDER);
    ui.visuals_mut().widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT);
}

fn text_field(ui: &mut egui::Ui, text: &mut String, hint: &str, width: f32) -> egui::Response {
    ui.scope(|ui| {
        input_colors(ui);
        ui.add(
            egui::TextEdit::singleline(text)
                .text_color(TEXT)
                .hint_text(RichText::new(hint).color(MUTED))
                .desired_width(width),
        )
    })
    .inner
}

fn path_editor(ui: &mut egui::Ui, text: &mut String, hint: &str) -> egui::Response {
    text_field(ui, text, hint, f32::INFINITY)
}

fn multiline_field(ui: &mut egui::Ui, text: &mut String) -> egui::Response {
    ui.scope(|ui| {
        input_colors(ui);
        ui.add(
            egui::TextEdit::multiline(text)
                .text_color(TEXT)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        )
    })
    .inner
}

fn check_box(ui: &mut egui::Ui, checked: &mut bool, label: &str) -> egui::Response {
    ui.scope(|ui| {
        // Keep the box dark and the check pale cyan, independently of button fills.
        let visuals = ui.visuals_mut();
        for widget in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.bg_fill = INPUT;
            widget.bg_stroke = egui::Stroke::new(1.0_f32, INPUT_BORDER);
            widget.fg_stroke = egui::Stroke::new(1.5_f32, CHECK_MARK);
            widget.corner_radius = egui::CornerRadius::same(2);
        }
        ui.checkbox(checked, RichText::new(label).color(TEXT))
    })
    .inner
}

struct PublisherApp {
    page: Page,
    profile: PathBuf,
    plugins: Vec<PluginRow>,
    selected_plugin: usize,
    engines: Vec<EngineRow>,
    select_discovered: bool,
    manual_engine: String,
    output: String,
    build: bool,
    binary: bool,
    low_memory_build: bool,
    max_parallel_actions: u32,
    dirty: bool,
    busy: Option<Work>,
    reporter: Reporter,
    tx: SyncSender<Message>,
    rx: Receiver<Message>,
    logs: VecDeque<String>,
    status: String,
    has_error: bool,
    completed: usize,
    total: usize,
    job: String,
    last_output: Option<PathBuf>,
    scan_path: String,
    scan_publisher: String,
    scan_year: String,
    scan_engine: String,
    scan_platforms: String,
    report: Option<Report>,
    severity_filter: Option<Severity>,
    search: String,
    missing_cjk: bool,
    logs_open: bool,
}

impl PublisherApp {
    fn new(cc: &eframe::CreationContext<'_>, profile: Option<PathBuf>) -> Self {
        let missing_cjk = !install_fonts(&cc.egui_ctx);
        // set_style alone only changes the currently selected theme. An OS theme
        // event can otherwise switch back to unstyled light controls on our dark panels.
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        let mut style = (*cc.egui_ctx.style_of(egui::Theme::Dark)).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.panel_fill = BACKGROUND;
        style.visuals.window_fill = SURFACE;
        style.visuals.window_stroke = egui::Stroke::new(1.0_f32, BORDER);
        style.visuals.faint_bg_color = SURFACE;
        style.visuals.extreme_bg_color = INPUT;
        style.visuals.code_bg_color = BACKGROUND;
        style.visuals.hyperlink_color = ACCENT;
        style.visuals.warn_fg_color = WARN;
        style.visuals.error_fg_color = ERROR;
        style.visuals.selection.bg_fill = SELECTED;
        style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
        style.visuals.text_cursor.stroke = egui::Stroke::new(1.5_f32, CHECK_MARK);
        style.visuals.collapsing_header_frame = false;
        style.visuals.window_highlight_topmost = false;
        let widgets = &mut style.visuals.widgets;
        for (widget, fill, stroke) in [
            (&mut widgets.noninteractive, SURFACE, BORDER),
            (&mut widgets.inactive, BUTTON, BORDER),
            (&mut widgets.hovered, BUTTON_HOVER, INPUT_BORDER),
            (&mut widgets.active, PRIMARY, ACCENT),
            (&mut widgets.open, Color32::from_rgb(42, 52, 59), BORDER),
        ] {
            widget.bg_fill = fill;
            widget.weak_bg_fill = fill;
            widget.bg_stroke = egui::Stroke::new(1.0_f32, stroke);
            widget.fg_stroke = egui::Stroke::new(1.0_f32, ICON);
            widget.corner_radius = egui::CornerRadius::same(4);
            widget.expansion = 0.0;
        }
        style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
        style.spacing.scroll.foreground_color = false;
        style.spacing.item_spacing = egui::vec2(7.0, 4.0);
        style.spacing.button_padding = egui::vec2(7.0, 3.0);
        style.spacing.interact_size.y = 22.0;
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(21.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        // Install the palette into both slots, including popup/dropdown styles.
        cc.egui_ctx.set_style_of(egui::Theme::Dark, style.clone());
        cc.egui_ctx.set_style_of(egui::Theme::Light, style);
        let (tx, rx) = mpsc::sync_channel(2048);
        let base = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let profile = profile.unwrap_or_else(|| base.join("publisher.json"));
        let mut app = Self {
            page: Page::Publish,
            profile,
            plugins: Vec::new(),
            selected_plugin: 0,
            engines: Vec::new(),
            select_discovered: true,
            manual_engine: String::new(),
            output: base.join("releases").display().to_string(),
            build: true,
            binary: true,
            low_memory_build: false,
            max_parallel_actions: config::default_parallel_actions(),
            dirty: false,
            busy: None,
            reporter: Reporter::default(),
            tx,
            rx,
            logs: VecDeque::new(),
            status: "就绪 · 添加插件，选择引擎版本后开始".into(),
            has_error: false,
            completed: 0,
            total: 0,
            job: String::new(),
            last_output: None,
            scan_path: String::new(),
            scan_publisher: String::new(),
            scan_year: String::new(),
            scan_engine: String::new(),
            scan_platforms: String::new(),
            report: None,
            severity_filter: None,
            search: String::new(),
            missing_cjk,
            logs_open: false,
        };
        if app.profile.exists() {
            let path = app.profile.clone();
            app.load_profile(&path);
        }
        app.discover(&cc.egui_ctx, None);
        app
    }

    fn log(&mut self, text: impl AsRef<str>) {
        for line in text.as_ref().lines() {
            if self.logs.len() >= 4000 {
                self.logs.pop_front();
            }
            self.logs.push_back(line.chars().take(4000).collect());
        }
    }
    fn error(&mut self, error: impl std::fmt::Display) {
        self.status = error.to_string();
        self.has_error = true;
        self.log(format!("错误：{}", self.status));
    }

    fn worker(
        &mut self,
        ctx: &egui::Context,
        work: Work,
        task: impl FnOnce(Reporter) -> Result<Product> + Send + 'static,
    ) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(work);
        self.has_error = false;
        self.completed = 0;
        self.total = 0;
        self.job.clear();
        self.status = "正在处理…".into();
        let tx = self.tx.clone();
        let repaint = ctx.clone();
        self.reporter = Reporter::new(move |event| {
            let _ = tx.send(Message::Event(event));
            repaint.request_repaint();
        });
        let reporter = self.reporter.clone();
        let tx = self.tx.clone();
        let repaint = ctx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("publisher-worker".into())
            .spawn(move || {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(reporter)))
                        .map_err(|_| "工作线程发生异常；请查看已保存日志。".to_string())
                        .and_then(|r| r.map_err(|e| format!("{e:#}")));
                let _ = tx.send(Message::Finished(result));
                repaint.request_repaint();
            })
        {
            self.busy = None;
            self.error(error);
        }
    }

    fn poll(&mut self) {
        // Bound each UI frame's work even when UAT produces many log lines.
        for _ in 0..512 {
            let Ok(message) = self.rx.try_recv() else {
                break;
            };
            match message {
                Message::Event(event) => match event {
                    Event::Log(text) => self.log(text),
                    Event::Status(status) => self.status = status,
                    Event::Progress {
                        completed,
                        total,
                        detail,
                    } => {
                        self.completed = completed;
                        self.total = total;
                        self.job = detail;
                    }
                    Event::Output(path) => self.last_output = Some(path),
                },
                Message::Finished(result) => {
                    let work = self.busy.take();
                    match result {
                        Ok(Product::Engines(engines)) => {
                            let count = engines.len();
                            for engine in engines {
                                self.merge_engine(engine);
                            }
                            self.select_discovered = false;
                            self.status = format!("发现 {count} 个 UE5 安装；可手动补充路径");
                        }
                        Ok(Product::Published) => {
                            self.status = if work == Some(Work::Preflight) {
                                "预检查通过，未构建或写入发布文件。"
                            } else {
                                "打包完成。点击“打开产物”查看 ZIP、日志和报告。"
                            }
                            .into();
                            self.log(self.status.clone());
                        }
                        Ok(Product::Inspected(report)) => {
                            self.has_error =
                                report.count(Severity::Error) > 0 || !report.scan_complete;
                            self.status = report.overall.clone();
                            self.report = Some(*report);
                        }
                        Err(error) => self.error(error),
                    }
                }
            }
        }
    }

    fn merge_engine(&mut self, engine: Engine) {
        if let Some(row) = self
            .engines
            .iter_mut()
            .find(|row| same(&row.root, &engine.root))
        {
            row.label = format!("UE {}", engine.label());
            row.root = engine.root;
        } else {
            self.engines.push(EngineRow {
                label: format!("UE {}", engine.label()),
                root: engine.root,
                selected: self.select_discovered,
            });
        }
    }
    fn discover(&mut self, ctx: &egui::Context, manual: Option<PathBuf>) {
        if manual.is_some() {
            self.dirty = true;
        }
        self.worker(ctx, Work::Discover, move |_| {
            Ok(Product::Engines(if let Some(path) = manual {
                vec![Engine::read(&path)?]
            } else {
                engines::discover(&[])
            }))
        });
    }

    fn load_profile(&mut self, path: &Path) {
        match config::load(path) {
            Ok(config) => {
                self.output = files::future_path(&config.output)
                    .unwrap_or(config.output)
                    .display()
                    .to_string();
                self.engines.clear();
                self.select_discovered = config.engines.is_empty();
                for path in config
                    .engines
                    .iter()
                    .chain(config.plugins.iter().flat_map(|p| p.engines.iter()))
                {
                    if self.engines.iter().any(|e| same(&e.root, path)) {
                        continue;
                    }
                    self.engines.push(EngineRow {
                        root: path.clone(),
                        label: path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into(),
                        selected: config.engines.iter().any(|p| same(p, path)),
                    });
                }
                self.build = config.build;
                self.binary = config.binary_archive;
                self.low_memory_build = config.low_memory_build;
                self.max_parallel_actions = config.max_parallel_actions;
                self.plugins = config.plugins.into_iter().map(PluginRow::from).collect();
                self.selected_plugin = 0;
                self.profile = path.to_path_buf();
                self.dirty = false;
                self.has_error = false;
                self.status = format!("已载入配置：{}", path.display());
                self.log(self.status.clone());
            }
            Err(error) => self.error(format!("载入配置失败：{error:#}")),
        }
    }

    fn configuration(&self) -> Result<config::Config> {
        if self.output.trim().is_empty() {
            bail!("请选择输出目录。");
        }
        let engines: Vec<_> = self
            .engines
            .iter()
            .filter(|e| e.selected)
            .map(|e| e.root.clone())
            .collect();
        if self.plugins.iter().any(|p| !p.custom_engines) && engines.is_empty() {
            bail!("请勾选默认引擎版本。单插件也可单独指定引擎。");
        }
        let mut plugins = Vec::new();
        for p in &self.plugins {
            if p.path.trim().is_empty() {
                bail!("插件路径不能为空。");
            }
            if p.custom_engines && p.engines.is_empty() {
                bail!("插件 {} 尚未选择专用引擎。", p.path);
            }
            plugins.push(config::Plugin {
                path: files::future_path(Path::new(p.path.trim()))?,
                platforms: list(&p.platforms),
                exclude: p
                    .exclude
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect(),
                engines: if p.custom_engines {
                    p.engines.clone()
                } else {
                    Vec::new()
                },
            });
        }
        let config = config::Config {
            output: files::future_path(Path::new(self.output.trim()))?,
            engines,
            plugins,
            build: self.build,
            binary_archive: self.binary,
            low_memory_build: self.low_memory_build,
            max_parallel_actions: self.max_parallel_actions,
        };
        config::validate(&config)?;
        Ok(config)
    }

    fn save(&mut self, choose: bool) {
        let path = if choose || !self.profile.exists() {
            rfd::FileDialog::new()
                .add_filter("配置 JSON", &["json"])
                .set_file_name("publisher.json")
                .save_file()
        } else {
            Some(self.profile.clone())
        };
        if let Some(path) = path {
            let result = self
                .configuration()
                .and_then(|config| config::save(&path, &config));
            match result {
                Ok(()) => {
                    self.profile = path;
                    self.dirty = false;
                    self.has_error = false;
                    self.status = "配置已保存；旧版本保留为同目录 .bak。".into();
                }
                Err(error) => self.error(format!("保存失败：{error:#}")),
            }
        }
    }

    fn add_plugin(&mut self, path: PathBuf) {
        match files::descriptor(&path) {
            Ok(path) => {
                if let Some(index) = self
                    .plugins
                    .iter()
                    .position(|p| same(Path::new(&p.path), &path))
                {
                    self.selected_plugin = index;
                    return;
                }
                let platforms = files::read_json(&path)
                    .ok()
                    .and_then(|d| d["SupportedTargetPlatforms"].as_array().cloned())
                    .map(|a| {
                        a.into_iter()
                            .filter_map(|p| p.as_str().map(String::from))
                            .collect::<Vec<_>>()
                    })
                    .filter(|a| !a.is_empty())
                    .unwrap_or_else(|| {
                        vec![if cfg!(windows) {
                            "Win64"
                        } else if cfg!(target_os = "macos") {
                            "Mac"
                        } else {
                            "Linux"
                        }
                        .into()]
                    });
                self.plugins.push(PluginRow {
                    path: path.display().to_string(),
                    platforms: platforms.join("+"),
                    exclude: String::new(),
                    custom_engines: false,
                    engines: Vec::new(),
                });
                self.selected_plugin = self.plugins.len() - 1;
                self.dirty = true;
            }
            Err(error) => self.error(format!("添加插件失败：{error:#}")),
        }
    }

    fn publish(&mut self, ctx: &egui::Context, preflight: bool) {
        match self.configuration() {
            Ok(config) => {
                if !preflight {
                    self.last_output = None;
                }
                self.worker(
                    ctx,
                    if preflight {
                        Work::Preflight
                    } else {
                        Work::Publish
                    },
                    move |reporter| {
                        package::release_with(&config, preflight, false, &reporter)?;
                        Ok(Product::Published)
                    },
                );
            }
            Err(error) => self.error(format!("配置不完整：{error:#}")),
        }
    }

    fn inspect(&mut self, ctx: &egui::Context) {
        let options = (|| -> Result<Options> {
            let year = if self.scan_year.trim().is_empty() {
                None
            } else {
                let year = self
                    .scan_year
                    .trim()
                    .parse::<u32>()
                    .context("发行年份应为四位数字")?;
                if !(1900..=2100).contains(&year) {
                    bail!("发行年份超出合理范围");
                }
                Some(year)
            };
            if self.scan_path.trim().is_empty() {
                bail!("请选择待上传的 ZIP 或插件目录");
            }
            Ok(Options {
                publisher: self.scan_publisher.trim().into(),
                year,
                engine: self.scan_engine.trim().into(),
                platforms: list(&self.scan_platforms),
            })
        })();
        match options {
            Ok(options) => {
                let path = PathBuf::from(self.scan_path.trim());
                self.report = None;
                self.worker(ctx, Work::Inspect, move |reporter| {
                    Ok(Product::Inspected(Box::new(scanner::scan(
                        &path, &options, &reporter,
                    )?)))
                });
            }
            Err(error) => self.error(error),
        }
    }

    fn top(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .frame(panel_frame().inner_margin(egui::Margin::symmetric(16, 12)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("UE5 Plugin Publisher");
                    ui.label(RichText::new("Ken An · 0.2").small().color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_enabled_ui(self.busy.is_none(), |ui| {
                            if action_button(ui, ActionIcon::Edit, "另存配置").clicked() {
                                self.save(true);
                            }
                            if action_button(ui, ActionIcon::Save, "保存配置").clicked() {
                                self.save(false);
                            }
                            if action_button(ui, ActionIcon::Folder, "打开配置").clicked() {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("配置 JSON", &["json"])
                                    .pick_file()
                                {
                                    self.load_profile(&path);
                                }
                            }
                        });
                    });
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    for (page, label) in
                        [(Page::Publish, "插件打包"), (Page::Inspect, "上传包检查")]
                    {
                        let selected = self.page == page;
                        if ui
                            .add(
                                egui::Button::new(RichText::new(label).color(if selected {
                                    TAB_TEXT
                                } else {
                                    MUTED
                                }))
                                .fill(if selected { TAB_SELECTED } else { SURFACE })
                                .stroke(egui::Stroke::NONE),
                            )
                            .clicked()
                        {
                            self.page = page;
                        }
                    }
                    ui.separator();
                    let name = self
                        .profile
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "{}{}",
                                name,
                                if self.dirty { " · 未保存" } else { "" }
                            ))
                            .small()
                            .color(MUTED),
                        )
                        .truncate(),
                    )
                    .on_hover_text(self.profile.display().to_string());
                });
            });
    }

    fn bottom(&mut self, ctx: &egui::Context) {
        let open = self.logs_open;
        egui::TopBottomPanel::bottom("activity")
            .frame(
                egui::Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(16, 12)),
            )
            .resizable(open)
            .default_height(if open { 170.0 } else { 56.0 })
            .min_height(if open { 120.0 } else { 56.0 })
            .max_height(if open { 360.0 } else { 56.0 })
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(if open { "收起日志" } else { "展开日志" })
                                    .color(MUTED),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        self.logs_open = !open;
                    }
                    if self.busy.is_some() {
                        ui.spinner();
                    }
                    let status_width =
                        (ui.available_width() - if open { 130.0 } else { 0.0 }).max(80.0);
                    ui.add_sized(
                        [status_width, 24.0],
                        egui::Label::new(
                            RichText::new(&self.status)
                                .small()
                                .color(if self.has_error { ERROR } else { MUTED }),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&self.status);
                    if open {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("清空").clicked() {
                                self.logs.clear();
                            }
                            if ui
                                .small_button("复制")
                                .on_hover_text("复制窗口中的最近 4000 行日志")
                                .clicked()
                            {
                                ctx.copy_text(
                                    self.logs.iter().cloned().collect::<Vec<_>>().join("\n"),
                                );
                            }
                        });
                    }
                });
                if open {
                    ui.separator();
                    egui::ScrollArea::both()
                        .id_salt("log_lines")
                        .stick_to_bottom(true)
                        .show_rows(ui, 17.0, self.logs.len(), |ui, range| {
                            for i in range {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&self.logs[i])
                                            .monospace()
                                            .size(12.0)
                                            .color(MUTED),
                                    )
                                    .wrap_mode(egui::TextWrapMode::Extend),
                                );
                            }
                        });
                }
            });
    }

    fn actions(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("actions")
            .frame(panel_frame())
            .show(ctx, |ui| {
                if self.total > 0 {
                    ui.add(
                        egui::ProgressBar::new(self.completed as f32 / self.total as f32)
                            .text(format!("{} / {}  {}", self.completed, self.total, self.job))
                            .fill(PRIMARY),
                    );
                    ui.add_space(3.0);
                }
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.strong(if self.page == Page::Publish {
                            "发布任务"
                        } else {
                            "检查任务"
                        });
                        let summary = if self.page == Page::Publish {
                            format!(
                                "{} 个插件 · {} 个默认引擎 · {}",
                                self.plugins.len(),
                                self.engines.iter().filter(|e| e.selected).count(),
                                if self.build {
                                    "编译验证后生成源码包"
                                } else {
                                    "仅源码快照"
                                }
                            )
                        } else {
                            "ZIP / 插件目录 · 文件与规范检查".into()
                        };
                        ui.label(RichText::new(summary).small().color(MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if matches!(self.busy, Some(Work::Publish) | Some(Work::Inspect)) {
                            if self.busy == Some(Work::Publish) && ui
                                .add_enabled(!self.reporter.force_stopping(),
                                    egui::Button::new(RichText::new("强制终止").color(ERROR)))
                                .on_hover_text("结束本工具启动的 UAT 及其子进程；复制或压缩步骤在下一个检查点退出。本次未完成产物不可提交。")
                                .clicked() {
                                self.reporter.stop_after_current.store(true, Ordering::Relaxed);
                                self.reporter.force_stop.store(true, Ordering::Relaxed);
                                self.status = "正在强制终止本次构建进程树…".into();
                                self.log(self.status.clone());
                            }
                            if ui
                                .add_enabled(
                                    !self.reporter.stopping(),
                                    egui::Button::new(if self.busy == Some(Work::Inspect) {
                                        "停止扫描"
                                    } else {
                                        "当前任务结束后停止"
                                    }),
                                )
                                .on_hover_text("正在执行的构建或压缩完成后，不再开始下一项")
                                .clicked()
                            {
                                self.reporter
                                    .stop_after_current
                                    .store(true, Ordering::Relaxed);
                                self.log("已请求停止；当前任务结束后，不再开始下一项。");
                            }
                        } else {
                            ui.add_enabled_ui(self.busy.is_none(), |ui| {
                                if self.page == Page::Publish {
                                    let label = if self.build {
                                        "编译验证后生成源码包"
                                    } else {
                                        "生成未验证源码包"
                                    };
                                    if ui
                                        .add_enabled(
                                            !self.plugins.is_empty(),
                                            primary_button(label),
                                        )
                                        .clicked()
                                    {
                                        self.publish(ctx, false);
                                    }
                                    if ui
                                        .add_enabled(
                                            !self.plugins.is_empty(),
                                            egui::Button::new("×  只做预检查"),
                                        )
                                        .clicked()
                                    {
                                        self.publish(ctx, true);
                                    }
                                } else if ui
                                    .add_enabled(
                                        !self.scan_path.trim().is_empty(),
                                        primary_button("开始扫描"),
                                    )
                                    .clicked()
                                {
                                    self.inspect(ctx);
                                }
                            });
                        }
                        if let Some(path) = self.last_output.clone() {
                            if ui.button("打开产物").clicked() {
                                if let Err(e) = open_folder(&path) {
                                    self.error(e);
                                }
                            }
                        }
                    });
                });
            });
    }

    fn publish_page(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("plugins")
            .resizable(true)
            .default_width(250.0)
            .width_range(220.0..=360.0)
            .frame(panel_frame())
            .show(ctx, |ui| {
                ui.add_enabled_ui(self.busy.is_none(), |ui| {
                    ui.label(RichText::new(format!("插件列表 {}", self.plugins.len())).size(18.0));
                    ui.label(
                        RichText::new("选择插件，编辑右侧发布设置")
                            .small()
                            .color(MUTED),
                    );
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("添加文件").clicked() {
                            if let Some(paths) = rfd::FileDialog::new()
                                .add_filter("UE 插件", &["uplugin"])
                                .pick_files()
                            {
                                for path in paths {
                                    self.add_plugin(path);
                                }
                            }
                        }
                        if ui.button("添加目录").clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                self.add_plugin(path);
                            }
                        }
                    });
                    ui.separator();
                    let list_height = (ui.available_height() - 66.0).max(80.0);
                    egui::ScrollArea::vertical()
                        .id_salt("plugin_list")
                        .max_height(list_height)
                        .show(ui, |ui| {
                            ui.set_min_height(list_height);
                            if self.plugins.is_empty() {
                                ui.add_space(24.0);
                                ui.label("尚未添加插件");
                                ui.label(
                                    RichText::new("添加 .uplugin 或目录\n也可将文件拖入窗口")
                                        .small()
                                        .color(MUTED),
                                );
                            }
                            for (i, plugin) in self.plugins.iter().enumerate() {
                                let title = Path::new(&plugin.path)
                                    .file_stem()
                                    .unwrap_or_default()
                                    .to_string_lossy();
                                let selected = self.selected_plugin == i;
                                egui::Frame::new()
                                    .fill(if selected { SELECTED } else { SURFACE })
                                    .corner_radius(6)
                                    .inner_margin(10)
                                    .show(ui, |ui| {
                                        let response = ui
                                            .add_sized(
                                                [ui.available_width(), 27.0],
                                                egui::Button::new(RichText::new(title))
                                                    .fill(if selected {
                                                        SELECTED_TITLE
                                                    } else {
                                                        CARD
                                                    })
                                                    .stroke(egui::Stroke::NONE)
                                                    .truncate(),
                                            )
                                            .on_hover_text(&plugin.path);
                                        if response.clicked() {
                                            self.selected_plugin = i;
                                        }
                                        ui.label(
                                            RichText::new(format!(
                                                "{} · {}",
                                                plugin.platforms,
                                                if plugin.custom_engines {
                                                    "独立引擎"
                                                } else {
                                                    "默认引擎"
                                                }
                                            ))
                                            .small()
                                            .color(MUTED),
                                        );
                                    });
                                ui.add_space(3.0);
                            }
                        });
                    ui.separator();
                    if ui
                        .add_enabled(!self.plugins.is_empty(), egui::Button::new("移除所选插件"))
                        .clicked()
                    {
                        self.plugins.remove(self.selected_plugin);
                        self.selected_plugin = self
                            .selected_plugin
                            .min(self.plugins.len().saturating_sub(1));
                        self.dirty = true;
                    }
                });
            });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BACKGROUND).inner_margin(20))
            .show(ctx, |ui| {
                ui.heading("打包设置");
                ui.label(RichText::new("依次确认插件、引擎与输出方式，在底部开始任务。").color(MUTED));
                ui.add_space(8.0);
                egui::ScrollArea::vertical().id_salt("publish_settings").auto_shrink([false, false]).show(ui, |ui| {
                    ui.add_enabled_ui(self.busy.is_none(), |ui| {
                        card(ui, "01  当前插件", "仅应用于左侧选中的插件", |ui| {
                            if let Some(plugin) = self.plugins.get_mut(self.selected_plugin) {
                                ui.label(RichText::new("插件文件 / 目录").small().color(MUTED));
                                self.dirty |= path_editor(ui, &mut plugin.path, "插件文件 / 目录").changed();
                                ui.label(RichText::new("目标平台").small().color(MUTED));
                                self.dirty |= path_editor(ui, &mut plugin.platforms, "Win64+Linux").changed();
                                self.dirty |= check_box(ui, &mut plugin.custom_engines, "为此插件单独选择引擎版本").changed();
                                if plugin.custom_engines {
                                    if self.engines.is_empty() { ui.label(RichText::new("请先在下方添加引擎。").color(MUTED)); }
                                    for engine in &self.engines {
                                        let mut selected = plugin.engines.iter().any(|p| same(p, &engine.root));
                                        if check_box(ui, &mut selected, &engine.label).on_hover_text(engine.root.display().to_string()).changed() {
                                            plugin.engines.retain(|p| !same(p, &engine.root));
                                            if selected { plugin.engines.push(engine.root.clone()); }
                                            self.dirty = true;
                                        }
                                    }
                                }
                                egui::CollapsingHeader::new("高级选项 · 排除文件").show(ui, |ui| {
                                    ui.label(RichText::new("每行一个相对 glob，例如 Docs/Internal/**；原文件保持不变。").small().color(MUTED));
                                    self.dirty |= multiline_field(ui, &mut plugin.exclude).changed();
                                });
                            } else {
                                ui.label(RichText::new("从左侧添加插件后，在这里设置平台与版本。").color(MUTED));
                            }
                        });
                        card(ui, "02  引擎版本", "默认用于所有插件；独立引擎设置优先", |ui| {
                            if ui.button("重新发现 UE5").clicked() { self.discover(ctx, None); }
                            for engine in &mut self.engines {
                                ui.horizontal(|ui| {
                                    if check_box(ui, &mut engine.selected, &engine.label).changed() {
                                        self.dirty = true;
                                        self.select_discovered = false;
                                    }
                                    ui.add(egui::Label::new(RichText::new(engine.root.display().to_string()).small().color(MUTED)).truncate())
                                        .on_hover_text(engine.root.display().to_string());
                                });
                            }
                            if self.engines.is_empty() { ui.label(RichText::new("尚未发现 UE5，可手动添加安装目录。").color(MUTED)); }
                            egui::CollapsingHeader::new("手动添加引擎").default_open(self.engines.is_empty()).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let width = (ui.available_width() - 160.0).max(100.0);
                                    text_field(ui, &mut self.manual_engine, "UE5 / Engine / RunUAT 路径", width);
                                    if ui.button("浏览").clicked() {
                                        if let Some(path) = rfd::FileDialog::new().pick_folder() { self.manual_engine = path.display().to_string(); }
                                    }
                                    if ui.button("添加引擎").clicked() && !self.manual_engine.trim().is_empty() {
                                        self.select_discovered = true;
                                        self.discover(ctx, Some(PathBuf::from(self.manual_engine.trim())));
                                    }
                                });
                            });
                        });
                        card(ui, "03  输出与打包", "使用独立源码快照；多版本兼容性由实际构建结果决定", |ui| {
                            ui.label(RichText::new("输出目录").small().color(MUTED));
                            ui.horizontal(|ui| {
                                let width = (ui.available_width() - 100.0).max(100.0);
                                self.dirty |= text_field(ui, &mut self.output, "输出目录", width).changed();
                                if action_button(ui, ActionIcon::Folder, "选择目录").clicked() {
                                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                        self.output = path.display().to_string(); self.dirty = true;
                                    }
                                }
                            });
                            ui.separator();
                            self.dirty |= check_box(ui, &mut self.build, "生成源码包前，逐版本编译验证插件").changed();
                            ui.add_enabled_ui(self.build, |ui| {
                                self.dirty |= check_box(ui, &mut self.low_memory_build, "低内存模式（关闭 UBA，限制编译并发）").changed();
                                if self.low_memory_build {
                                    egui::CollapsingHeader::new(format!("并发设置 · 当前上限 {}", self.max_parallel_actions)).id_salt("memory_options").show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label("编译并发上限");
                                            self.dirty |= ui.add(egui::DragValue::new(&mut self.max_parallel_actions).range(1..=64)).changed();
                                            ui.label(RichText::new("建议 2；仍内存不足时设为 1").small().color(MUTED));
                                        });
                                        ui.label(RichText::new("仅影响本次子进程；需要引擎支持 UBT_EXTRA_ARGS。").small().color(MUTED));
                                    });
                                }
                            });
                            ui.add_enabled_ui(self.build, |ui| {
                                self.dirty |= check_box(ui, &mut self.binary, "同时生成二进制安装 ZIP").changed();
                            });
                            if !self.build {
                                ui.label(RichText::new("仅生成 Source-UNVALIDATED.zip，不执行 UE 构建。").small().color(WARN));
                            }
                        });
                    });
                });
            });
    }

    fn inspect_page(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("inspection_options")
            .resizable(true)
            .default_width(300.0)
            .width_range(260.0..=400.0)
            .frame(panel_frame())
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("scan_settings")
                    .show(ui, |ui| {
                        ui.label(RichText::new("检查对象").size(17.0).strong());
                        ui.label(
                            RichText::new("选择待上传的源码包或插件目录")
                                .small()
                                .color(MUTED),
                        );
                        ui.add_space(8.0);
                        ui.add_enabled_ui(self.busy.is_none(), |ui| {
                            path_editor(ui, &mut self.scan_path, "ZIP / .uplugin / 插件目录");
                            ui.horizontal(|ui| {
                                if ui.button("选择文件").clicked() {
                                    if let Some(path) = rfd::FileDialog::new()
                                        .add_filter("上传包 / 插件", &["zip", "uplugin"])
                                        .pick_file()
                                    {
                                        self.scan_path = path.display().to_string();
                                    }
                                }
                                if ui.button("选择目录").clicked() {
                                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                        self.scan_path = path.display().to_string();
                                    }
                                }
                            });
                            ui.add_space(12.0);
                            ui.separator();
                            ui.strong("预期发布信息");
                            ui.label(
                                RichText::new("选填；留空时采用通用检查")
                                    .small()
                                    .color(MUTED),
                            );
                            for (label, value, hint) in [
                                ("版权人", &mut self.scan_publisher, "例如 Ken An"),
                                ("发行年份", &mut self.scan_year, "例如 2026"),
                                ("引擎版本", &mut self.scan_engine, "例如 5.8.0"),
                                ("目标平台", &mut self.scan_platforms, "例如 Win64+Linux"),
                            ] {
                                ui.label(RichText::new(label).small().color(MUTED));
                                path_editor(ui, value, hint);
                            }
                        });
                        ui.add_space(14.0);
                        ui.separator();
                        ui.strong("检查依据");
                        ui.label(
                            RichText::new(
                                "规则核对 2026-11-18\n官方更新 2026-09-21\n包含本次 TRC 反馈",
                            )
                            .small()
                            .color(MUTED),
                        );
                        ui.hyperlink_to("Fab 技术要求", scanner::TECHNICAL);
                        ui.hyperlink_to("文件结构 / Python", scanner::STRUCTURE);
                    });
            });
        let mut export = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(20))
            .show(ctx, |ui| {
                ui.heading("检查结果");
                ui.label(RichText::new("定位文件问题，查看处理建议与待验证项目。").color(MUTED));
                ui.add_space(8.0);
                if let Some(report) = &self.report {
                    card(ui, "扫描概览", "", |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(report.input.display().to_string())
                                    .small()
                                    .color(MUTED),
                            )
                            .truncate(),
                        )
                        .on_hover_text(report.input.display().to_string());
                        ui.horizontal_wrapped(|ui| {
                            for severity in [
                                Severity::Error,
                                Severity::Warning,
                                Severity::Manual,
                                Severity::Pass,
                            ] {
                                egui::Frame::new()
                                    .fill(BACKGROUND)
                                    .corner_radius(5)
                                    .inner_margin(8)
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(report.count(severity).to_string())
                                                    .size(18.0)
                                                    .color(color(severity)),
                                            );
                                            ui.label(
                                                RichText::new(severity.label())
                                                    .small()
                                                    .color(MUTED),
                                            );
                                        });
                                    });
                            }
                        });
                        ui.label(&report.overall);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!("{} 个文件", report.file_count))
                                    .small()
                                    .color(MUTED),
                            );
                            if ui.small_button("导出 JSON").clicked() {
                                export = Some(false);
                            }
                            if ui.small_button("导出 Markdown").clicked() {
                                export = Some(true);
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("severity")
                            .selected_text(
                                self.severity_filter
                                    .map(Severity::label)
                                    .unwrap_or("全部结果"),
                            )
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.severity_filter, None, "全部结果");
                                for s in [
                                    Severity::Error,
                                    Severity::Warning,
                                    Severity::Manual,
                                    Severity::Pass,
                                ] {
                                    ui.selectable_value(
                                        &mut self.severity_filter,
                                        Some(s),
                                        s.label(),
                                    );
                                }
                            });
                        let width = ui.available_width();
                        text_field(ui, &mut self.search, "搜索规则、文件名或问题", width);
                    });
                    ui.add_space(4.0);
                    let query = self.search.to_lowercase();
                    let mut visible = 0;
                    egui::ScrollArea::vertical()
                        .id_salt("inspection_findings")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (i, finding) in
                                report.findings.iter().enumerate().filter(|(_, f)| {
                                    self.severity_filter.map_or(true, |s| f.severity == s)
                                        && (query.is_empty()
                                            || format!("{} {} {}", f.rule, f.path, f.detail)
                                                .to_lowercase()
                                                .contains(&query))
                                })
                            {
                                visible += 1;
                                egui::Frame::new()
                                    .fill(SURFACE)
                                    .stroke(egui::Stroke::new(1.0_f32, BORDER))
                                    .corner_radius(6)
                                    .inner_margin(12)
                                    .show(ui, |ui| {
                                        ui.set_min_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(finding.severity.label())
                                                    .small()
                                                    .color(color(finding.severity)),
                                            );
                                            ui.label(RichText::new(&finding.rule).strong());
                                        });
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&finding.path).small().color(MUTED),
                                            )
                                            .truncate(),
                                        )
                                        .on_hover_text(&finding.path);
                                        ui.add(egui::Label::new(&finding.detail).truncate())
                                            .on_hover_text(&finding.detail);
                                        egui::CollapsingHeader::new("问题详情与建议")
                                            .id_salt(("finding", i))
                                            .show(ui, |ui| {
                                                ui.label(&finding.detail);
                                                ui.label(format!("建议：{}", finding.suggestion));
                                                ui.label(
                                                    RichText::new(format!(
                                                        "依据：{}",
                                                        finding.basis
                                                    ))
                                                    .small()
                                                    .color(MUTED),
                                                );
                                            });
                                    });
                                ui.add_space(5.0);
                            }
                            if visible == 0 {
                                ui.label(
                                    RichText::new("没有匹配的结果，请调整筛选条件。").color(MUTED),
                                );
                            }
                        });
                } else {
                    card(
                        ui,
                        if self.busy == Some(Work::Inspect) {
                            "正在扫描"
                        } else {
                            "等待开始检查"
                        },
                        "",
                        |ui| {
                            ui.add_space(18.0);
                            ui.label("在左侧选择 ZIP 或插件目录，然后点击底部“开始扫描”。");
                            ui.label(
                                RichText::new("支持直接拖入 ZIP；扫描不需要安装 Unreal Engine。")
                                    .small()
                                    .color(MUTED),
                            );
                            ui.add_space(18.0);
                        },
                    );
                }
            });
        if let Some(markdown) = export {
            self.export_report(markdown);
        }
    }

    fn export_report(&mut self, markdown: bool) {
        let Some(report) = &self.report else {
            return;
        };
        let ext = if markdown { "md" } else { "json" };
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("检查报告", &[ext])
            .set_file_name(format!("PluginInspection.{ext}"))
            .save_file()
        {
            let result = if markdown {
                Ok(report.markdown())
            } else {
                serde_json::to_string_pretty(report)
            };
            match result
                .map_err(anyhow::Error::from)
                .and_then(|text| std::fs::write(&path, text).map_err(anyhow::Error::from))
            {
                Ok(()) => {
                    self.status = format!("检查报告已导出：{}", path.display());
                    self.log(self.status.clone());
                }
                Err(error) => self.error(error),
            }
        }
    }
}

impl eframe::App for PublisherApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        if self.busy.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            if ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.status = "任务仍在进行，请等待完成或先停止后续任务，再关闭窗口。".into();
            }
        }
        if self.busy.is_none() {
            let dropped: Vec<_> = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .filter_map(|f| f.path.clone())
                    .collect()
            });
            for path in dropped {
                if self.page == Page::Inspect
                    || path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
                {
                    self.scan_path = path.display().to_string();
                    self.page = Page::Inspect;
                } else {
                    self.add_plugin(path);
                }
            }
        }
        self.top(ctx);
        self.bottom(ctx);
        self.actions(ctx);
        if self.missing_cjk {
            egui::TopBottomPanel::top("font_warning").show(ctx, |ui| { ui.colored_label(WARN, "CJK font missing. Install a Chinese font (Microsoft YaHei / Noto Sans CJK) and restart."); });
        }
        match self.page {
            Page::Publish => self.publish_page(ctx),
            Page::Inspect => self.inspect_page(ctx),
        }
    }
}

fn install_fonts(ctx: &egui::Context) -> bool {
    let mut candidates = Vec::new();
    if let Some(root) = std::env::var_os("WINDIR") {
        candidates.push(PathBuf::from(&root).join("Fonts/msyh.ttc"));
        candidates.push(PathBuf::from(root).join("Fonts/simhei.ttf"));
    }
    candidates.extend(
        [
            "/System/Library/Fonts/PingFang.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        ]
        .map(PathBuf::from),
    );
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("cjk".into(), egui::FontData::from_owned(bytes).into());
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push("cjk".into());
            }
            ctx.set_fonts(fonts);
            return true;
        }
    }
    false
}

fn open_folder(path: &Path) -> Result<()> {
    #[cfg(windows)]
    let mut command = std::process::Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command.arg(path).spawn().context("无法打开输出目录")?;
    Ok(())
}

pub fn run(profile: Option<PathBuf>) -> std::result::Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1220.0, 860.0])
            .with_min_inner_size([960.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "UE5 Plugin Publisher",
        options,
        Box::new(move |cc| Ok(Box::new(PublisherApp::new(cc, profile)))),
    )
    .map_err(|e| e.to_string())
}
