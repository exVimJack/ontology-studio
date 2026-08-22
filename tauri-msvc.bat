@echo off
REM Run `npx tauri ...` under MSVC dev env so cargo + DLLs are available.
REM Usage: tauri-msvc.bat <tauri args...>
setlocal
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
cd /d "%~dp0"
npx @tauri-apps/cli %*
endlocal
