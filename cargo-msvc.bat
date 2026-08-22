@echo off
REM Run cargo under the MSVC developer environment so cc finds INCLUDE/LIB.
REM Also ensures WebView2Loader.dll is in the deps dir (needed to run any
REM tauri-linked exe / test from raw cargo; `tauri dev`/`tauri build` handle
REM this automatically, but bare cargo test/run does not).
REM Usage: cargo-msvc.bat <cargo args...>
setlocal
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul

REM Copy WebView2Loader.dll into target deps if missing (webview2-com-sys ships it).
REM The exact registry hash dir varies; glob-match the x64 dll.
for /f "delims=" %%D in ('dir /b /s "%USERPROFILE%\.cargo\registry\src\webview2-com-sys*\x64\WebView2Loader.dll" 2^>nul') do (
    if exist "C:\Users\think\AppData\Local\onto-studio-target\debug\deps" (
        if not exist "C:\Users\think\AppData\Local\onto-studio-target\debug\deps\WebView2Loader.dll" copy "%%D" "C:\Users\think\AppData\Local\onto-studio-target\debug\deps\" >nul 2>&1
    )
)

cargo %*
endlocal
