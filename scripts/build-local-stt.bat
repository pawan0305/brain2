@echo off
REM Build Brain2 with the on-device Whisper STT backend (cargo feature: local-stt).
REM
REM Toolchain (all no-admin, via Scoop):  scoop install llvm ninja
REM   - llvm provides libclang.dll, which whisper-rs-sys needs for bindgen.
REM   - ninja is the CMake generator (see below).
REM
REM Why the env vars below:
REM   * CMAKE_GENERATOR=Ninja  - the VS/MSBuild generator dies on whisper.cpp's
REM     deeply-nested vulkan-shaders-gen ExternalProject (MAX_PATH / .tlog).
REM     Ninja sidesteps that machinery entirely (and is faster).
REM   * CARGO_TARGET_DIR=C:\b\bt - a SHORT build path so the deep whisper.cpp
REM     paths stay under Windows' 260-char limit. The proper long-paths fix
REM     needs admin (HKLM), which this corporate laptop doesn't have, so we
REM     keep the build artifacts at a short root instead.
REM   * LIBCLANG_PATH - where bindgen finds libclang.dll.
REM
REM Pass extra cargo args after the script (e.g. `build-local-stt.bat --release`).
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
set "CMAKE_GENERATOR=Ninja"
set "LIBCLANG_PATH=%USERPROFILE%\scoop\apps\llvm\current\bin"
set "CARGO_TARGET_DIR=C:\b\bt"
set "PATH=%USERPROFILE%\scoop\shims;%VULKAN_SDK%\Bin;%PATH%"
cargo build --features local-stt --manifest-path "%~dp0..\src-tauri\Cargo.toml" %*
echo === build-local-stt done (exit %errorlevel%) ===
