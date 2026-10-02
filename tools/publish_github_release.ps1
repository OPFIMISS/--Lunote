param(
    [string]$Tag = "v2.0.0",
    [string]$Repository = "OPFIMISS/--Lunote",
    [switch]$Draft
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$version = $Tag.TrimStart('v')
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Tag must have format vMAJOR.MINOR.PATCH' }
$notes = Join-Path $root "RELEASE_NOTES_$version.md"
$assets = @(
    (Join-Path $root "dist/Lunote-$version.apk"),
    (Join-Path $root "dist/Lunote-$version-windows-x64-setup.exe"),
    (Join-Path $root "dist/Lunote-$version-windows-x64-portable.exe"),
    (Join-Path $root "dist/Lunote-$version-windows-x64.zip")
)

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    throw 'GitHub CLI gh is required. Install gh and run gh auth login first.'
}
foreach ($asset in $assets) {
    if (-not (Test-Path -LiteralPath $asset)) { throw "Asset not found: $asset" }
}
if (-not (Test-Path $notes)) { throw "Release notes not found: $notes" }

$tagExists = git -C $root tag --list $Tag
if ($tagExists -ne $Tag) { throw "Local tag not found: $Tag" }

$sums = Join-Path $root "dist/Lunote-$version-SHA256SUMS.txt"
$lines = foreach ($asset in $assets) {
    $hash = (Get-FileHash -LiteralPath $asset -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($asset))"
}
$lines | Set-Content -LiteralPath $sums -Encoding ASCII
$assets += $sums

gh release view $Tag --repo $Repository *> $null
if ($LASTEXITCODE -eq 0) {
    gh release upload $Tag @assets --repo $Repository --clobber
    if ($LASTEXITCODE -ne 0) { throw 'Release asset upload failed' }
    gh release edit $Tag --repo $Repository --notes-file $notes --title "月笺 Lunote $Tag"
} else {
    $arguments = @('release', 'create', $Tag) + $assets + @('--repo', $Repository,
        '--title', "月笺 Lunote $Tag", '--notes-file', $notes, '--verify-tag')
    if ($Draft) { $arguments += '--draft' }
    gh @arguments
}
if ($LASTEXITCODE -ne 0) { throw 'Release publication failed' }
Write-Output "Release $Tag published to $Repository"
$lines | Write-Output
