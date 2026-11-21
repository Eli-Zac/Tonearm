# Builds Tonearm's core for Android into app/src/main/jniLibs, and writes
# its Kotlin side into app/src/main/java/uniffi. Run before Gradle, and again
# whenever the Rust changes.
#
#   .\build-rust.ps1            the emulator (x86_64) and phones (arm64)
#   .\build-rust.ps1 -Emulator  the emulator only, quicker
#   .\build-rust.ps1 -Release   fully optimized, for a build to hand out
#   .\build-rust.ps1 -Arm64     phones only (what the release APK holds)
#
# Needs the Android SDK and NDK (ANDROID_HOME, default C:\Android\Sdk),
# cargo-ndk, and the targets x86_64-linux-android and aarch64-linux-android.
param([switch]$Emulator, [switch]$Release, [switch]$Arm64)
# Not 'Stop': cargo writes its progress to stderr, which Windows PowerShell
# would take for a failure. Exit codes say whether a step failed.
$ErrorActionPreference = 'Continue'

$root = Split-Path $PSScriptRoot -Parent
if (-not $env:ANDROID_HOME) { $env:ANDROID_HOME = 'C:\Android\Sdk' }
if (-not $env:ANDROID_NDK_HOME) {
    $env:ANDROID_NDK_HOME = (Get-ChildItem "$env:ANDROID_HOME\ndk" -Directory | Sort-Object Name | Select-Object -Last 1).FullName
}
# QuickJS's bindings are generated for Android with the NDK's own libclang.
if (-not $env:LIBCLANG_PATH) {
    $env:LIBCLANG_PATH = Join-Path $env:ANDROID_NDK_HOME 'toolchains\llvm\prebuilt\windows-x86_64\bin'
}
# It loads libxml2 and libwinpthread from beside it, which Windows looks
# for on the PATH.
$env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
# The NDK's headers want the Android version in the target, as Gradle's
# minSdk (26) has it. cargo-ndk sets the underscored variable without one,
# so this sets the dashed name, which bindgen reads first, in full.
$sysroot = (Join-Path $env:ANDROID_NDK_HOME 'toolchains\llvm\prebuilt\windows-x86_64\sysroot') -replace '\\', '/'
foreach ($triple in 'x86_64-linux-android', 'aarch64-linux-android') {
    [Environment]::SetEnvironmentVariable(
        "BINDGEN_EXTRA_CLANG_ARGS_$triple",
        "--target=${triple}26 --sysroot=$sysroot -I$sysroot/usr/include/$triple",
        'Process')
}
# Casting opens Tonearm's own receiver (song, cover, lyrics on a screen),
# as the desktop's test builds do. Devices that can't open it fall back to
# Google's Default Media Receiver.
if (-not $env:TONEARM_CAST_APP) { $env:TONEARM_CAST_APP = '79497171' }
# Builds for Android are kept off D:, which is short of room.
if (-not $env:TONEARM_ANDROID_TARGET) { $env:TONEARM_ANDROID_TARGET = 'C:\Android\target' }
$target = $env:TONEARM_ANDROID_TARGET

$abis = if ($Emulator) { @('x86_64') } elseif ($Arm64) { @('arm64-v8a') } else { @('x86_64', 'arm64-v8a') }
$flags = $abis | ForEach-Object { '-t', $_ }
$jni = Join-Path $PSScriptRoot 'app\src\main\jniLibs'
# Day to day the `fast` profile: optimized, but quick to build. -Release
# gives the release profile's whole program optimization, for a build to
# hand out.
$profile = if ($Release) { 'release' } else { 'fast' }

Push-Location $root
try {
    cargo ndk @flags -P 26 -o $jni build --profile $profile -p tonearm-android --target-dir $target
    if ($LASTEXITCODE -ne 0) { throw 'cargo ndk failed' }

    # The Kotlin side comes from this computer's build of the same library:
    # the release profile strips the description UniFFI reads.
    cargo build -p tonearm-android --lib --target-dir $target
    if ($LASTEXITCODE -ne 0) { throw 'cargo build of the bindings source failed' }
    $library = Join-Path $target 'debug\tonearm_android.dll'
    # Writes app\src\main\java\uniffi\tonearm_android, beside the app's own.
    $out = Join-Path $PSScriptRoot 'app\src\main\java'
    cargo run -p tonearm-android --bin uniffi-bindgen --target-dir $target -- generate --library $library --language kotlin --out-dir $out --no-format
    if ($LASTEXITCODE -ne 0) { throw 'uniffi-bindgen failed' }
} finally {
    Pop-Location
}
