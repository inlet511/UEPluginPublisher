@echo off
rem Copyright 2026 Ken An. All Rights Reserved.
setlocal
set "PUBLISHER_EXE=%~dp0target\release\ue-plugin-publisher.exe"
if not exist "%PUBLISHER_EXE%" set "PUBLISHER_EXE=%~dp0ue-plugin-publisher.exe"
if not exist "%PUBLISHER_EXE%" (
    echo Build the Rust tool first: cargo build --release --manifest-path "%~dp0Cargo.toml"
    pause
    exit /b 1
)
set "PUBLISHER_PROFILE=%~dp0publisher.json"
if not exist "%PUBLISHER_PROFILE%" if exist "%~dp0target\release\publisher.json" set "PUBLISHER_PROFILE=%~dp0target\release\publisher.json"
start "" "%PUBLISHER_EXE%" --profile "%PUBLISHER_PROFILE%"
exit /b 0
