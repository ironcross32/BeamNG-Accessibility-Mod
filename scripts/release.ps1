<#
.SYNOPSIS
    Cut a BEAM release: bump the workspace version, tag it, push the tag.

.DESCRIPTION
    Invoked as `git release` (alias: !powershell.exe -NoProfile -ExecutionPolicy Bypass
    -File scripts/release.ps1). The version and the tag are one contract:
    .github/workflows/release.yml refuses to build a tag whose name disagrees with the
    version (`cargo xtask check-tag`), and a shipped build compares its APP_VERSION
    (which is the workspace version, CARGO_PKG_VERSION) against the release tag to decide
    whether it is out of date. Bumping one by hand and forgetting the other is silent in
    the worst way: every client concludes it is up to date and the release reaches
    nobody. So this does both, in the one order that works.

    The version lives in the root Cargo.toml, `[workspace.package] version`, and every
    workspace crate's entry in Cargo.lock carries it too. Both are rewritten and
    committed together: CI builds with --locked, so a stale Cargo.lock fails the release.

    The bump is COMMITTED before the tag is created. A tag is a pointer at a commit, so
    tagging with the new version still in the working tree would tag the OLD one, and
    CI, which checks out the tag, would fail the build it was asked to make.

    Only Cargo.toml and Cargo.lock are committed. This repo routinely carries unrelated
    work in progress, and a release commit that swept it up would be both a surprise and
    impossible to revert cleanly. For the same reason, either file already having
    uncommitted changes stops the release before anything is touched.
#>

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

function Fail($message) {
    Write-Host "release: $message" -ForegroundColor Red
    exit 1
}

# Enter accepts the default, so a run with no console would silently take it and push
# a release nobody asked for. Refuse rather than guess.
if ([Console]::IsInputRedirected) {
    Fail "this must be run interactively -- it prompts for the version."
}

# --- Locate the repo and the version --------------------------------------

$repo = (& git rev-parse --show-toplevel 2>$null)
if ($LASTEXITCODE -ne 0 -or -not $repo) { Fail "not inside a git repository." }
$repo = $repo.Trim()
Set-Location $repo

$manifestPath = Join-Path $repo 'Cargo.toml'
$lockPath = Join-Path $repo 'Cargo.lock'
if (-not (Test-Path $manifestPath)) { Fail "Cargo.toml not found at $manifestPath." }

# The first `version = "..."` after the [workspace.package] header, before any other
# section header.
$pattern = '(?ms)^(\[workspace\.package\][^\[]*?^version\s*=\s*")([^"]+)(")'
$content = [System.IO.File]::ReadAllText($manifestPath)
$match = [regex]::Match($content, $pattern)
if (-not $match.Success) { Fail "[workspace.package] version not found in Cargo.toml." }
$current = $match.Groups[2].Value

$dirty = (& git status --porcelain -- Cargo.toml Cargo.lock)
if ($dirty) {
    Fail "Cargo.toml or Cargo.lock has uncommitted changes; commit or stash them first. Nothing has been changed."
}

# --- Work out the default next version ------------------------------------
# Increment the last numeric component. A pre-release suffix is dropped rather than
# incremented: "0.2.0-rc1" defaults to "0.2.1", because there is no sensible way to
# guess whether the next one is rc2 or the real thing.

$core = ($current -split '[-+]')[0]
$parts = $core -split '\.'
$default = $null
if ($parts.Count -ge 1 -and ($parts[-1] -match '^\d+$')) {
    $bumped = [int]$parts[-1] + 1
    $parts[-1] = "$bumped"
    $default = ($parts -join '.')
}

# --- Ask ------------------------------------------------------------------

Write-Host ""
Write-Host "Current version: $current"
if ($default) {
    $answer = Read-Host "Version to release [$default]"
} else {
    Write-Host "(cannot derive a default from '$current' -- type the version in full)"
    $answer = Read-Host "Version to release"
}
if ([string]::IsNullOrWhiteSpace($answer)) { $answer = $default }
if ([string]::IsNullOrWhiteSpace($answer)) { Fail "no version given." }

# Accept a leading v so a typed "v0.2.0" does the obvious thing rather than producing a
# tag called "vv0.2.0".
$version = $answer.Trim()
$version = $version -replace '^[vV]', ''

# Cargo requires full semver: three numeric components, optional pre-release.
if ($version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
    Fail "'$version' is not a MAJOR.MINOR.PATCH version (Cargo requires all three)."
}

$tag = "v$version"

# --- Conflict checks ------------------------------------------------------
# Local tags first, then the remote's, because a tag that exists only on origin still
# makes the push fail -- and it fails AFTER the commit has been made, which is the state
# worth not being in.

& git fetch --tags --quiet 2>$null | Out-Null

$existing = (& git tag --list $tag)
if ($existing) { Fail "tag $tag already exists locally. Nothing has been changed." }

$remote = (& git ls-remote --tags origin "refs/tags/$tag" 2>$null)
if ($LASTEXITCODE -eq 0 -and $remote) {
    Fail "tag $tag already exists on origin. Nothing has been changed."
}

if ($version -eq $current) {
    Fail "the version is already $current and no tag exists for it. Pick a new version, or tag $tag by hand."
}

$branch = (& git rev-parse --abbrev-ref HEAD).Trim()
if ($branch -eq 'HEAD') { Fail "HEAD is detached; check out a branch first." }

# --- Write the version ----------------------------------------------------

Write-Host ""
Write-Host "Releasing $tag (version $current -> $version) from branch $branch"

$updated = [regex]::Replace($content, $pattern, "`${1}$version`${3}", 1)
if ($updated -eq $content) { Fail "failed to rewrite the version." }
# No BOM: Cargo reads the manifest as UTF-8 and a BOM is noise in the diff.
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($manifestPath, $updated, $utf8NoBom)
Write-Host "  updated Cargo.toml"

# Moves only the workspace crates' own entries; --offline so no dependency is re-resolved.
& cargo update --workspace --offline --quiet
if ($LASTEXITCODE -ne 0) {
    & git checkout -- $manifestPath
    Fail "cargo update failed; Cargo.toml has been restored and nothing was committed."
}
Write-Host "  refreshed Cargo.lock"

# --- Commit, tag, push ----------------------------------------------------

& git add -- $manifestPath $lockPath
if ($LASTEXITCODE -ne 0) { Fail "git add failed." }

& git commit -m "Release $tag" -- $manifestPath $lockPath
if ($LASTEXITCODE -ne 0) { Fail "git commit failed. Cargo.toml and Cargo.lock have been edited but nothing was tagged." }
Write-Host "  committed the version bump"

& git tag -a $tag -m "Release $tag"
if ($LASTEXITCODE -ne 0) { Fail "git tag failed. The bump is committed; tag it by hand." }
Write-Host "  created annotated tag $tag"

# The branch goes first. Pushing a tag alone uploads the objects it needs but leaves
# origin's branch behind it, so the release commit would exist on the remote with
# nothing pointing at it but the tag.
& git push origin $branch
if ($LASTEXITCODE -ne 0) { Fail "pushing $branch failed. The tag exists locally; push it once the branch is up." }

& git push origin $tag
if ($LASTEXITCODE -ne 0) { Fail "pushing $tag failed. Run: git push origin $tag" }

Write-Host ""
Write-Host "Pushed $tag. The release workflow builds and publishes it:" -ForegroundColor Green
Write-Host "  https://github.com/ironcross32/BeamNG-Accessibility-Mod/actions"
