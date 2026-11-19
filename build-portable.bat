@echo off
setlocal
title SpotifySync - Build Rust

where cargo >nul 2>&1
if errorlevel 1 (
    echo Rust/Cargo nao encontrado. Instale via https://rustup.rs
    exit /b 1
)

set "VCVARS="
if exist "%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" (
    for /f "usebackq tokens=*" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VCVARS=%%i\VC\Auxiliary\Build\vcvars64.bat"
)

if defined VCVARS if exist "%VCVARS%" call "%VCVARS%" >nul

cargo build --release
if errorlevel 1 exit /b %errorlevel%

if not exist dist mkdir dist
copy /y target\release\SpotifySync.exe dist\SpotifySync.exe >nul
if errorlevel 1 exit /b %errorlevel%

echo Build concluido: dist\SpotifySync.exe
