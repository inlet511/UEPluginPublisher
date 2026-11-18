# Working on UEPluginPublisher

- The user handles compilation, tests, and execution. Do not run Cargo build/check/test/run, UBT/UAT, UnrealEditor, launcher scripts, or background builds unless the user explicitly authorizes them.
- Source/config/documentation edits and reading code/logs are allowed. Static formatting checks with rustfmt are allowed.
- Keep the tool independent of any particular UE plugin, workspace, or engine path. Use portable placeholders in public examples.
- Preserve personal profiles and existing generated artifacts. Never commit target/, personal profiles, plugin submission ZIPs, or local logs.
- Keep copyright notices as Copyright 2026 Ken An. All Rights Reserved. Preserve the existing licensing choice unless the user requests a change.
- Describe verification accurately; formatting checks do not confirm compilation or runtime behavior.
