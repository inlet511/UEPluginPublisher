# UE5 Plugin Publisher

[简体中文使用说明](README.zh-CN.md) · [检查规则 / Inspection rules](CHECK_RULES.zh-CN.md)

A Rust desktop tool for preparing Unreal Engine 5 code plugin submissions. Manage multiple plugins and UE installations, optionally compile each plugin against its selected engines, create source ZIPs, and inspect an existing submission package.

The tool runs independently of any particular plugin or Unreal project. It prepares local files; it does **not** upload to Fab or guarantee approval.

## Features

- Dark desktop UI with native file dialogs, drag and drop, reusable JSON profiles, live logs, and fixed task controls.
- Automatic UE5 discovery through Epic Launcher manifests, the Windows registry, common installation directories, and environment variables. Manual selection supports installed and source engines.
- Multiple plugins and engine versions, with per-plugin platforms, engine overrides, and exclusion patterns.
- Optional `RunUAT BuildPlugin -StrictIncludes` compilation validation before creating a source submission ZIP.
- Optional binary installation ZIPs, SHA256 inventories, archive verification, and JSON packaging reports.
- Low-memory compilation mode with a configurable concurrency limit for engines supporting `UBT_EXTRA_ARGS`.
- Stop after the current task, or force-stop the UAT process tree started by the tool.
- Read-only inspection of a prepared ZIP, `.uplugin`, or plugin folder, with searchable findings and JSON / Markdown exports. Inspection does not require UE.

## Build and launch

Use current stable Rust. The manifest declares Rust 1.85 as the source baseline; the committed dependency versions can require a newer compiler. On Windows, install the MSVC build tools required by Rust. Compiling **plugins** additionally requires the selected UE version's C++ toolchain and platform SDKs.

```powershell
git clone git@github.com:inlet511/UEPluginPublisher.git
cd UEPluginPublisher
cargo build --release --locked
.\target\release\ue-plugin-publisher.exe
```

`Publish.cmd` and `Publish-Saved.cmd` also launch the GUI after building. They do not start plugin compilation automatically. Windows is the primary target; macOS/Linux code paths are present but have not received equivalent platform validation. The current UI is in Simplified Chinese.

The two executables are:

| Executable | Purpose |
| --- | --- |
| `ue-plugin-publisher` | Desktop UI; optionally accepts `--profile <path>` |
| `ue-plugin-publisher-cli` | Engine discovery, interactive profile setup, inspection, and batch packaging |

## Typical workflow

1. Add one or more plugin descriptors or plugin directories.
2. Select discovered engines, or add an installation manually. Configure platforms and optional engine overrides for each plugin.
3. Select an output folder outside the plugin and engine directories.
4. Run the precheck, then choose compilation validation and generate the source ZIP. Binary ZIP generation is optional.
5. Inspect the resulting source ZIP on the inspection page and address errors, warnings, and manual verification items.
6. Submit the intended source package through Fab yourself, with the required listing information and supporting documentation.

For a C++ code plugin, a source submission includes its `.uplugin`, `Source`, and required assets, shaders, configuration, and resources. Local compilation products are kept out of the source ZIP. Compilation validates compatibility; it is not a requirement to include your local DLLs in that source submission.

| Output | Meaning |
| --- | --- |
| `*-Fab-Source.zip` | Source archive produced after this run's compilation and file verification succeed |
| `*-Source-UNVALIDATED.zip` | Source archive created without compilation validation in this run |
| `*-Binary.zip` | Optional precompiled installation package for the selected engine/platform |
| `PackagingReport.json` | Successful task details and artifact paths |
| `Failure.json` / `Stopped.json` | Failed or interrupted work; unfinished artifacts must not be submitted |

## Profiles and CLI

Copy `publisher.example.json` to your own `publisher.json` and change all placeholder paths. Relative paths resolve against the profile's directory. Personal profiles, outputs, logs, and build artifacts are ignored by Git. `volume-heatmap.example.json` illustrates a single plugin using low-memory mode; the example plugin itself is not included.

```powershell
# Discover engines.
.\target\release\ue-plugin-publisher-cli.exe engines

# Check a profile without writing output or starting UE build processes.
.\target\release\ue-plugin-publisher-cli.exe release --config .\publisher.json --dry-run

# Compile and package according to the profile.
.\target\release\ue-plugin-publisher-cli.exe release --config .\publisher.json

# Package source without compilation.
.\target\release\ue-plugin-publisher-cli.exe release --config .\publisher.json --source-only

# Inspect an existing submission without running or modifying its contents.
.\target\release\ue-plugin-publisher-cli.exe inspect D:\Releases\MyPlugin.zip --publisher "Your Name" --year 2026 --engine 5.8.0 --platforms Win64 --report D:\Reports\Inspection.json
```

## Memory pressure and stopping

`UbaSessionServer ... Killed process ... Low on memory` means UBA is terminating compilation work under memory pressure. Repeated retries can make a build very slow. Stop the task, free memory, and enable low-memory mode with a concurrency limit of `2`, or `1` if necessary.

Low-memory mode changes only the spawned build's environment. It disables UBA and other remote executors and sets `-MaxParallelActions`. It checks the selected engine's UBT source for the supported environment-variable entry point; unsupported installations report an error instead of silently ignoring the setting. It does not edit global UE configuration.

Strict include validation disables PCH and Unity compilation and can be substantially slower than an editor incremental build. Disabling the binary ZIP only skips that archive; it does not skip compilation validation.

During a build, **强制终止** force-stops the tool's UAT process tree. **当前任务结束后停止** waits for the current task to finish. Copying, hashing, and ZIP operations stop at their next cancellation checkpoint. Logs and completed tasks are retained.

## Inspection scope

The bundled rules target **UE5 C++ code plugin source submissions**, not full projects, UEFN products, or content-only asset packs. The rules were checked on **2026-11-18** against the [Fab technical requirements](https://www.fab.com/o/technical-requirements), [file-format and structure requirements](https://dev.epicgames.com/documentation/en-us/fab/asset-file-format-and-structure-requirements-in-fab), and a plugin review feedback document. That feedback PDF is not distributed or required to use this repository.

Findings distinguish errors, warnings, manual verification, and passes. Zero errors does not prove runtime compatibility, correct third-party licensing, complete compliance, or Fab acceptance. See [the rule inventory and limits](CHECK_RULES.zh-CN.md).

## Repository layout

```text
src/gui.rs       Desktop interface
src/cli.rs       Command-line interface
src/config.rs    Profiles and validation
src/engines.rs   UE discovery
src/package.rs   Snapshot, compilation, packaging, and reporting
src/scanner.rs   Read-only submission inspection
src/files.rs     File inventories, hashing, and ZIP helpers
src/progress.rs  Worker events and stop requests
```

## Development and status

`Cargo.lock` is committed for reproducible application dependency resolution. Do not commit personal profiles, plugin sources, build output, or submission archives. The latest theme changes have received static formatting review; this repository upload does not imply a fresh build or runtime verification. Build and validate on the engines/platforms you intend to support.

Report reproducible problems through [GitHub Issues](https://github.com/inlet511/UEPluginPublisher/issues), including the tool version, OS, UE version, and relevant log excerpts. Remove private paths and other sensitive information before posting logs.

## Copyright

Copyright 2026 Ken An. All rights reserved. Source publication does not grant an open-source license; see [LICENSE](LICENSE). Third-party dependencies retain their own licenses. This is an independent tool and is not affiliated with or endorsed by Epic Games.
