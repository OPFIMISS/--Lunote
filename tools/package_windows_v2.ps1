param(
    [string]$Version = '2.0.0',
    [string]$ReleaseDir = (Join-Path $PSScriptRoot '../app/build/windows/x64/runner/Release'),
    [string]$OutDir = (Join-Path $PSScriptRoot '../dist')
)
$ErrorActionPreference = 'Stop'
$release = (Resolve-Path -LiteralPath $ReleaseDir).Path
foreach ($required in @('lunote_app.exe', 'lunote_bridge.dll', 'flutter_windows.dll', 'data/flutter_assets')) {
    if (-not (Test-Path -LiteralPath (Join-Path $release $required))) { throw "Incomplete Windows bundle: $required" }
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$output = (Resolve-Path -LiteralPath $OutDir).Path
$zip = Join-Path $output "Lunote-$Version-windows-x64.zip"
Compress-Archive -Path (Join-Path $release '*') -DestinationPath $zip -Force
$compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
$source = Join-Path $PSScriptRoot 'win_package/PackageLauncher.cs'
$icon = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../app/windows/runner/resources/app_icon.ico')).Path
foreach ($mode in @('portable', 'setup')) {
    $target = Join-Path $output "Lunote-$Version-windows-x64-$mode.exe"
    $arguments = @('/nologo', '/target:winexe', '/platform:x64', '/optimize+',
        '/reference:System.IO.Compression.dll', '/reference:System.IO.Compression.FileSystem.dll',
        '/reference:System.Windows.Forms.dll', '/reference:Microsoft.CSharp.dll',
        "/resource:$zip,PAYLOAD", "/win32icon:$icon", "/out:$target")
    if ($mode -eq 'setup') { $arguments += '/define:SETUP' }
    & $compiler @arguments $source
    if ($LASTEXITCODE -ne 0) { throw "Failed to compile $mode package" }
    Get-Item -LiteralPath $target | Select-Object Name, Length
}
Get-Item -LiteralPath $zip | Select-Object Name, Length
