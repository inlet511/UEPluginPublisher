# UE5 Plugin Publisher

[English](README.md) · [检查规则](CHECK_RULES.zh-CN.md) · [GitHub](https://github.com/inlet511/UEPluginPublisher)

通用 Rust 桌面发布工具，版权人 Ken An。默认启动图形界面，使用 egui/eframe 与原生文件选择框；支持多个独立插件、多个 UE5 安装版本、单插件平台/引擎设置、实时日志和独立的上传包检查页。工具不依赖特定插件名、源码位置或示例工程；命令行入口单独保留。

界面采用炭灰底色、青蓝选中态和紧凑控件，配置按钮配有线条图标。打包页按“插件列表 → 当前插件 → 引擎版本 → 输出与打包”组织；检查页左侧设置检查对象与预期信息，右侧显示结果概览、筛选和问题详情。主要操作固定在底部，日志默认收起，可展开并调整高度；长路径悬停可查看完整内容。低内存模式的并发参数可展开设置，构建期间保留固定的“强制终止”按钮。界面改动需手动重新编译后才会出现在 EXE 中。

两个页面统一使用固定暗色主题，避免系统浅色主题使按钮、输入框变白或标题变黑。普通按钮为灰底，输入框与复选框为近黑底，选中项、勾选和主操作使用青蓝色；正文使用亮色，辅助说明、路径、占位提示和日志使用更清晰的浅灰蓝，辅助字号为 12。下拉菜单、折叠项、进度条沿用同一套颜色。

## 第一次使用

安装当前 Rust stable（本工具源码最低 1.85；依赖的实际最低版本以 Cargo 提示为准）及对应 UE5 版本要求的 C++ 工具链、平台 SDK。随后由你手动执行：

```powershell
git clone git@github.com:inlet511/UEPluginPublisher.git
cd UEPluginPublisher
cargo build --release --locked
```

编译后双击 `target/release/ue-plugin-publisher.exe` 或 `Publish.cmd` 打开桌面界面。`Publish-Saved.cmd` 作为兼容入口，同样载入 `publisher.json` 后显示界面，不会在打开窗口时自动开始构建。

- **插件打包**：左侧添加多个 `.uplugin` / 插件目录，也可拖入；勾选自动发现的 UE5，或浏览/输入安装路径添加。每个插件可继承默认引擎，也可单独勾选版本，并编辑目标平台及排除 glob。选输出目录后点击“只做预检查”或“编译验证后生成源码包”。关闭“生成源码包前，逐版本编译验证插件”则只生成未验证源码包。
- **配置**：通过“打开配置 / 保存配置 / 另存配置”复用任务；保存已有配置会保留同目录带时间戳的 `.bak`。启动时只载入 EXE 同目录的 `publisher.json`，或 `--profile` 指定的文件；不存在则显示空白配置。加载配置不会开始构建。
- **任务进度**：底部显示已完成任务数、当前阶段与实时日志，完成后可打开产物目录。进度按插件/引擎任务数统计，不伪造单次编译的完成百分比。完整 UAT 日志保存在产物目录，窗口保留最近 4000 行。
- **停止**：“当前任务结束后停止”等待当前构建/压缩完成，不再开始下一项。“强制终止”则在构建期间结束本工具启动的 UAT 及其子进程（Windows 使用指定 PID 的 `taskkill /T /F`；macOS/Linux 使用独立进程组）；复制、校验、压缩阶段在下一检查点退出。保留日志、失败记录和已完成任务，未完成产物不可提交。终止失败会保留运行状态并允许重试。扫描可在文件间停止。任务进行中窗口会阻止直接关闭。
- **内存不足**：出现 `UbaSessionServer ... Low on memory` 时，UBA 正在因内存压力终止编译进程，反复重试可能很慢。可强制终止后勾选“低内存模式”，编译并发上限先设为 `2`，仍紧张则设为 `1`，并关闭不必要的编辑器及其他高内存程序。该模式通过本次子进程的 `UBT_EXTRA_ARGS` 关闭 UBA 等远程执行器，改用受并发上限约束的本地编译；不修改系统环境变量或全局 UE 配置。工具检查所选引擎源码是否支持此入口（本机 UE 5.8 已静态核对）；缺失或不支持时会报错，请关闭该选项后使用引擎自己的 UBT 配置。已有环境变量中的冲突并发/UBA 参数需先移除。

“编译验证”使用 `BuildPlugin -StrictIncludes`，会关闭 PCH 与 Unity 编译，并检查编辑器及适用的 Development / Shipping 目标，耗时通常高于编辑器内的增量编译。Fab 提交使用源码 ZIP，二进制安装 ZIP 是可选产物；关闭二进制 ZIP 只省去它的压缩步骤，不会跳过编译验证。

两个程序分别为 `ue-plugin-publisher.exe`（图形界面）和 `ue-plugin-publisher-cli.exe`（终端批处理）。可以把 EXE 和入口脚本复制到独立目录使用，不依赖本仓库。Windows 界面读取系统微软雅黑等中文字体，不把字体复制到插件包中；未找到中文字体时会显示英文提示。

仓库提供源码、入口脚本和已锁定的依赖清单，不附带本机构建产物。最新界面修改仅完成静态格式检查，本次仓库发布未重新编译、运行工具或验证真实构建任务；请在自己的目标引擎和平台上完成编译与验收。Windows 是主要使用平台，macOS/Linux 路径尚未经过同等验证。

## 上传包检查

切换到“上传包检查”，选择准备提交的 ZIP、`.uplugin` 或插件目录，点击“开始扫描”。可以把 ZIP 拖进窗口直接定位到该页。扫描不需要安装 UE，不执行插件代码，也不解压或改写原包；单独点击导出时才写入 JSON / Markdown 报告。

可选填写预期版权人、发行年份、`5.x.0` 引擎版本和平台。例如检查 VolumeHeatmap 示例可填 `Ken An` / `2026` / `5.8.0` / `Win64`；不填时采用通用检查，不强制把其他插件的作者认作 Ken An。

结果分为 **错误、提醒、待验证、通过**，支持筛选和文件/规则搜索。每条结果包含问题路径、原因、建议和规则来源。JSON 报告还保存输入文件清单、字节数、SHA256、扫描完整性、规则集版本、来源与核对日期。没有错误只表示已实现的静态规则未发现错误，**不表示全部规范合格或 Fab 审核通过**。

规则依据为 2026-11-18 查阅的 [Fab 技术要求](https://www.fab.com/o/technical-requirements)（页面标注 2026-09-21 更新）、[官方文件格式与结构要求](https://dev.epicgames.com/documentation/en-us/fab/asset-file-format-and-structure-requirements-in-fab)，以及一份插件审核反馈 `Volume Heatmap - UE Full TRC.pdf`（不随仓库分发，使用工具无需该文件）。具体规则、自动化范围与限制见 [检查规则说明](CHECK_RULES.zh-CN.md)。规则集随工具源码固定，不会自动抓网页或宣称永久匹配最新要求。

当前检查范围是 **UE5 代码插件源码提交包**，并非完整 UE 项目、UEFN 或纯内容资产包，不套用这些类型不同的目录/路径规则。二进制安装包中的 `Binaries` / `Intermediate` 会被报为源码提交问题，这是两种包用途不同造成的正常结果。

```powershell
# 命令行检查现成上传 ZIP；不构建、不修改原包。
.\target\release\ue-plugin-publisher-cli.exe inspect D:\Releases\YourPlugin.zip --publisher "Ken An" --year 2026 --engine 5.8.0 --platforms Win64 --report D:\Reports\Inspection.json
```

命令行遇到错误或扫描不完整返回非零；提醒/待验证不会伪装成全部审核通过。`--report` 只创建新文件，不覆盖旧报告。

## 自动发现与手动选择 UE5

自动发现来源包括 Epic Launcher 的 `LauncherInstalled.dat`、安装清单、Windows 的 Launcher 安装注册表和源码引擎注册表、`Program Files/Epic Games`，以及 `UE_ENGINE_ROOT` / `UE_ROOT` / `UNREAL_ENGINE_ROOT` 环境变量。读取实际 `Engine/Build/Build.version`，不会只靠文件夹名称猜版本。源码引擎也可手动输入路径。

```powershell
.\target\release\ue-plugin-publisher-cli.exe engines
.\target\release\ue-plugin-publisher-cli.exe engines --search D:\EpicGames --search E:\Unreal
.\target\release\ue-plugin-publisher-cli.exe wizard --profile .\publisher.json
```

手动路径可指向安装根目录、`Engine` 目录或 `RunUAT.bat`。无法发现时可在界面中浏览或手动输入。相同小版本的两个不同引擎安装可分别构建，输出目录有独立任务编号。

## 配置与批处理

`publisher.example.json` 展示两个插件、两个引擎及单插件引擎覆盖。所有相对路径以配置文件所在目录为基准；保存的向导配置使用绝对路径。

```powershell
# 只做静态预检查；不创建文件、不启动 UAT。
.\target\release\ue-plugin-publisher-cli.exe release --config .\publisher.json --dry-run

# 一键处理配置内的全部插件和版本。build=true 会编译 UE 插件。
.\target\release\ue-plugin-publisher-cli.exe release --config .\publisher.json

# 只生成明确标注 UNVALIDATED 的源码快照 ZIP，不编译 UE。
.\target\release\ue-plugin-publisher-cli.exe release --config .\publisher.json --source-only

# 单插件示例：先修改路径；示例插件源码不包含在本仓库中。
.\target\release\ue-plugin-publisher-cli.exe release --config .\volume-heatmap.example.json --dry-run
```

配置字段：

VolumeHeatmap 示例通过 `Tools/**` 演示排除内部工具目录；请按你的插件实际情况修改。通用界面和向导不猜测任意插件的草稿目录；请用 `exclude` 指定应排除的内部文件。

| 字段 | 含义 |
| --- | --- |
| `output` | 输出根目录，不能与任何插件源码或引擎安装目录互相包含 |
| `engines` | 全局引擎路径列表；空列表自动选择所有发现的 UE5 |
| `plugins[].path` | `.uplugin` 或只包含一个根描述符的插件目录 |
| `plugins[].engines` | 可选；覆盖本插件的全局引擎选择 |
| `plugins[].platforms` | 平台，如 `Win64`、`Linux`、`Mac`；默认当前宿主平台 |
| `plugins[].exclude` | 可选；插件内相对 glob，使用 `/`，如 `Docs/Internal/**` |
| `build` | 默认 `true`；使用每个引擎的 `RunUAT BuildPlugin` 构建 |
| `binary_archive` | 默认 `true`；构建通过后另外保存二进制安装 ZIP |
| `low_memory_build` | 默认 `false`；编译验证时使用本次进程专用的低内存选项，需引擎支持 `UBT_EXTRA_ARGS` |
| `max_parallel_actions` | 默认 `2`，范围 `1–64`；仅低内存模式生效 |

自动发现表示可以找到安装和安排构建，**不保证任意插件能跨 UE5 小版本编译**。源码 API 和资产兼容性仍由每个引擎构建结果与人工验收决定。示例中的 UE 5.8 / Win64 只是配置值，不会因工具支持多个版本就自动扩大插件的兼容性声明。

平台名称直接传给 UAT，构建环境必须安装其 SDK。比如 Windows 下不能凭选择 `Mac` 就获得可用的 Mac 构建；请在支持对应平台构建的宿主机执行。主要使用入口面向 Windows，Rust 源码也提供 macOS/Linux 的 RunUAT.sh 调用分支，但本次未在这些宿主机验证。

## 打包流程与输出

1. 检查每个插件仅含一个描述符、作者已填写、目标平台未超出插件声明、源码开头有含年份的 copyright 注释、Python 位于 `Content/Python`、包含插件目录名的相对文件路径不超过 170 字符。
2. 复制独立源码快照，记录复制前后 SHA256。默认排除根目录 `Binaries`、`Build`、`Intermediate`、`Saved`、`DerivedDataCache`、`HostProject`、`target`，以及隐藏目录、Python 缓存、`node_modules` 和常见本地临时文件。`Source/ThirdParty` 内真实依赖库会保留，不按二进制扩展名一刀切删除。链接/junction 必须替换成真实发布文件。
3. **只在快照中**写入对应 `EngineVersion`、`Installed=false`；保留现有模块平台限制，转换旧平台字段，缺少限制的模块才补充所选平台。补充 `Config/FilterPlugin.ini`，明确包含 Shaders、Config、Content、Resources 及其他随包自定义目录/根文件。
4. 调用该引擎的 `RunUAT BuildPlugin -Rocket -StrictIncludes -TargetPlatforms=...`。完整输出进入独立日志，同时显示到控制台。不启动编辑器、游戏、Cook 或自动化测试。
5. 核对 UAT 输出中的每个输入文件均存在且内容一致（描述符允许 UAT 设置安装字段，但引擎和模块字段须一致）。成功后压缩快照为源码提交 ZIP；可选压缩 UAT 结果为二进制 ZIP。回读 ZIP 全部文件并核对 SHA256，生成 `.sha256` 和 JSON 报告。

每次新建发布目录，不覆盖历史发布、不删除或同步工作插件；也不会自动从另一份示例工程挑选资产。应先自行保存、整理要发布的主插件内容。源码快照可用 `exclude` 排除内部文件；工具不会自动判断未知自定义文件是否应该公开。

快照新增的默认过滤包含规则放在原规则之前，避免覆盖发布者已有的排除规则。若原 `FilterPlugin.ini` 排除了仍在快照中的文件，构建后的完整性核对会失败；把这些文件同时加入配置的 `exclude` 后再发布。纯源码模式不会执行 UAT 的过滤器，因此发布范围以 `exclude` 为准。

```text
releases/release-<唯一编号>/
  ReleaseReport.json
  001-YourPlugin-UE5.x.y/
    Snapshot/YourPlugin/...
    UAT/...                         # 仅 build=true
    BuildPlugin.log                 # 仅执行 UAT 时
    OriginalSourceManifest.json
    SourceManifest.json
    PackagingReport.json            # 成功
    Failure.json                    # 失败时保存原因
    YourPlugin-UE5.x.y-Fab-Source.zip
    YourPlugin-UE5.x.y-Win64-Binary.zip
    *.zip.sha256
```

ZIP 根目录直接包含一个 `.uplugin` 及其 `Source`、`Content` 等目录。解压到 `Plugins/YourPlugin` 安装。`Fab-Source.zip` 不含本地构建目录，供源码提交；`Binary.zip` 保留 UAT 生成的 `Binaries` / `Intermediate` 等预编译内容，供对应引擎/平台本机安装。

`build=false` 或 `--source-only` 输出 `Source-UNVALIDATED.zip`，报告中 `build_succeeded=false`，不会生成二进制包。某个构建失败后继续其他插件/版本，最终整体返回非零退出码，并保留失败目录和日志；应以成功任务的 `PackagingReport.json` 和顶层 `ReleaseReport.json` 为准，不使用失败任务中可能残留的产物。

工具执行的是本地构建和归档，**不会上传或提交 Fab，也不承诺自动通过市场审核**。商品页、第三方资源声明、外部插件依赖安装、文档/支持链接、安装与运行验收仍需发布者处理；`BuildPlugin` 通过不等于运行测试通过。为避免 Windows 批处理转义问题，引擎和发布路径不接受 `% ! & | < > ^` 等字符；空格和中文路径可用。

## 版权与反馈

Copyright 2026 Ken An. All Rights Reserved. 当前保留所有权利，公开源码不等于授予开源许可，详见 [LICENSE](LICENSE)。第三方依赖遵循各自许可证。工具与 Epic Games 无隶属或官方背书关系。

问题可通过 [GitHub Issues](https://github.com/inlet511/UEPluginPublisher/issues) 反馈，请附工具版本、操作系统、引擎版本及已去除私人信息的日志片段。版本变化见 [CHANGELOG.md](CHANGELOG.md)。
