@echo off
REM Build a RUNNABLE Brain2 with local Whisper STT (bundled debug build).
REM
REM Why not just `cargo build`? That makes a DEV-mode binary that loads the
REM frontend from the Vite dev server (http://localhost:1420). Launched on its
REM own, it shows "localhost refused to connect". `tauri build --debug` runs in
REM prod mode and EMBEDS the built frontend, so the exe runs standalone.
REM
REM Output:
REM   C:\b\bt\debug\brain2.exe                      (runnable, standalone)
REM   C:\b\bt\debug\bundle\nsis\Brain2_*-setup.exe   (installer)
REM
REM Toolchain (no-admin): scoop install llvm ninja. See build-local-stt.bat.
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
set "CMAKE_GENERATOR=Ninja"
set "LIBCLANG_PATH=%USERPROFILE%\scoop\apps\llvm\current\bin"
set "CARGO_TARGET_DIR=C:\b\bt"
set "PATH=%USERPROFILE%\scoop\shims;%VULKAN_SDK%\Bin;%PATH%"
pushd "%~dp0.."
call npm run tauri build -- --debug --features local-stt
set CODE=%errorlevel%
popd
echo === run-local-stt build done (exit %CODE%) ===
