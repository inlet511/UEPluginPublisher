// Copyright 2026 Ken An. All Rights Reserved.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod gui;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "UE5 Plugin Publisher · 桌面发布工具")]
struct Args {
    /// Load a profile without starting a build.
    #[arg(long)]
    profile: Option<PathBuf>,
}

fn main() {
    // The GUI Windows subsystem has no console; show argument errors in a dialog.
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(error) => {
            rfd::MessageDialog::new()
                .set_title("UE5 Plugin Publisher")
                .set_description(error.to_string())
                .show();
            return;
        }
    };
    if let Err(error) = gui::run(args.profile) {
        rfd::MessageDialog::new()
            .set_title("无法打开发布工具")
            .set_level(rfd::MessageLevel::Error)
            .set_description(error)
            .show();
    }
}
