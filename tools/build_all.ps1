# 月笺 Lunote 一键构建：Windows 核心 + 桌面 + Android .so + APK
$ErrorActionPreference = 'Stop'
$root = 'D:\Lunote 2\moonletter'
$env:CARGO_HOME = 'D:\Lunote 2\.toolchains\cargo-home'
$env:CARGO_TARGET_DIR = 'D:\Lunote 2\.toolchains\rust-target'
$env:ANDROID_HOME = 'D:\Android'
$env:ANDROID_NDK_HOME = 'D:\Android\ndk\28.2.13676358'
$env:GRADLE_USER_HOME = 'D:\Lunote 2\.toolchains\gradle-home'
$env:PUB_HOSTED_URL = 'https://pub.flutter-io.cn'
$flutter = 'D:\Lunote 2\.toolchains\flutter\flutter\bin\flutter.bat'

Write-Output '=== [1/4] Windows 核心 lunote_bridge.dll ==='
Set-Location $root
cargo build --manifest-path Cargo.toml -p lunote-bridge --release
if ($LASTEXITCODE -ne 0) { Write-Output 'FAIL bridge'; exit 1 }
Copy-Item "$env:CARGO_TARGET_DIR\release\lunote_bridge.dll" "$root\app\build\windows\x64\runner\Release\lunote_bridge.dll" -Force
Write-Output 'OK bridge.dll'

Write-Output '=== [2/4] Windows 桌面 ==='
Set-Location "$root\app"
& $flutter build windows --release
if ($LASTEXITCODE -ne 0) { Write-Output 'FAIL windows'; exit 1 }
Copy-Item "$env:CARGO_TARGET_DIR\release\lunote_bridge.dll" "$root\app\build\windows\x64\runner\Release\lunote_bridge.dll" -Force
Write-Output 'OK windows'

Write-Output '=== [3/4] Android .so ==='
Set-Location "$root\crates\lunote-bridge"
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -o "$root\app\android\app\src\main\jniLibs" build --release
if ($LASTEXITCODE -ne 0) { Write-Output 'FAIL ndk'; exit 1 }
Write-Output 'OK ndk .so'

Write-Output '=== [4/4] Android APK ==='
Set-Location "$root\app"
& $flutter build apk --release
if ($LASTEXITCODE -ne 0) { Write-Output 'FAIL apk'; exit 1 }

# 最后再强制同步一次 dll（flutter 构建可能重置 Release 目录）
Copy-Item "$env:CARGO_TARGET_DIR\release\lunote_bridge.dll" "$root\app\build\windows\x64\runner\Release\lunote_bridge.dll" -Force

Write-Output '=== [5/5] 发布产物统一输出到 dist ==='
$dist = "$root\dist"
$distWin = "$dist\月笺"
Remove-Item $distWin -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $distWin | Out-Null
Copy-Item "$root\app\build\windows\x64\runner\Release\*" $distWin -Recurse -Force
Copy-Item "$root\app\build\app\outputs\flutter-apk\app-release.apk" "$dist\月笺.apk" -Force
Get-Item "$dist\月笺\lunote_app.exe","$dist\月笺.apk" | ForEach-Object { "dist: $($_.Name) $([math]::Round($_.Length/1MB,1))MB" }

Write-Output '=== 全部完成 ==='
