@echo off
REM Build the NICE PORTABLE Brain2 exe: RELEASE (optimized) + local Whisper STT.
REM Same toolchain as run-local-stt.bat (Ninja + libclang + Vulkan, no-admin),
REM but a release build — smaller, faster, suitable for distribution.
REM
REM Output:
REM   C:\b\bt\release\brain2.exe                          (portable, standalone)
REM   C:\b\bt\release\bundle\nsis\Brain2_*-setup.exe       (installer)
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
set "CMAKE_GENERATOR=Ninja"
set "LIBCLANG_PATH=%USERPROFILE%\scoop\apps\llvm\current\bin"
set "CARGO_TARGET_DIR=C:\b\bt"
set "PATH=%USERPROFILE%\scoop\shims;%VULKAN_SDK%\Bin;%PATH%"
pushd "%~dp0.."
call npm run tauri build -- --features local-stt
set CODE=%errorlevel%
popd
echo === build-portable (release) done (exit %CODE%) ===
