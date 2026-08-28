param(
    [string]$Tag = "v1.2.0",
    [string]$Repository = "OPFIMISS/--Lunote"
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$apk = Join-Path $root 'dist\月笺.apk'
$notes = Join-Path $root 'RELEASE_NOTES_1.2.0.md'

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    throw '未找到 GitHub CLI（gh）。请安装 gh 并执行 gh auth login 后重试。'
}
if (-not (Test-Path $apk)) { throw "APK 不存在: $apk" }
if (-not (Test-Path $notes)) { throw "发布说明不存在: $notes" }

$tagExists = git tag --list $Tag
if ($tagExists -ne $Tag) { throw "本地 tag 不存在: $Tag" }

$hash = (Get-FileHash -LiteralPath $apk -Algorithm SHA256).Hash
$body = Get-Content -LiteralPath $notes -Raw -Encoding UTF8
$body += [Environment]::NewLine + [Environment]::NewLine + "APK SHA-256: $hash"
$tmp = Join-Path $env:TEMP "lunote-release-$Tag.md"
Set-Content -LiteralPath $tmp -Value $body -Encoding UTF8

gh release view $Tag --repo $Repository *> $null
if ($LASTEXITCODE -eq 0) {
    gh release upload $Tag $apk --repo $Repository --clobber
    gh release edit $Tag --repo $Repository --notes-file $tmp --title "月笺 Lunote $Tag"
} else {
    gh release create $Tag $apk --repo $Repository --title "月笺 Lunote $Tag" --notes-file $tmp --verify-tag
}
Write-Output "Release $Tag published to $Repository"
Write-Output "APK SHA-256: $hash"
