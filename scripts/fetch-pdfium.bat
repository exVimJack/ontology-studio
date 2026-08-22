@echo off
REM 下载预编译 PDFium 动态库（决策 5）。Windows 版。
REM 版本必须与 crates/ingest/Cargo.toml 的 pdfium-render pdfium_7881 feature 一致。
REM 依赖 curl（Win10+ 自带）+ tar（Win10+ 自带）。

setlocal enabledelayedexpansion
set "VERSION=chromium/7881"
set "SCRIPT_DIR=%~dp0"
REM 项目根：脚本位于 <root>\scripts\，资源位于 <root>\src-tauri\resources\pdfium
set "RES_DIR=%~dp0..\src-tauri\resources\pdfium"
set "BASE=https://github.com/bblanchon/pdfium-binaries/releases/download"

if defined GH_PROXY set "BASE=%GH_PROXY%/https://github.com/bblanchon/pdfium-binaries/releases/download"

set "OUT=%RES_DIR%\win-x64"
if not exist "%OUT%" mkdir "%OUT%"
echo Downloading pdfium-win-x64 (chromium/7881)...
curl -L --fail --retry 2 -o "%TEMP%\pdfium-win.tgz" "%BASE%/%VERSION%/pdfium-win-x64.tgz" || goto :err
mkdir "%TEMP%\pdfium-win-x" 2>nul
tar xzf "%TEMP%\pdfium-win.tgz" -C "%TEMP%\pdfium-win-x" bin/
move /Y "%TEMP%\pdfium-win-x\bin\pdfium.dll" "%OUT%\pdfium.dll" >nul
rd /s /q "%TEMP%\pdfium-win-x"
del "%TEMP%\pdfium-win.tgz"
echo Done: %OUT%\pdfium.dll
exit /b 0

:err
echo Download failed.
exit /b 1
