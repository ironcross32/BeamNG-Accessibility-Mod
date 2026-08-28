<#
.SYNOPSIS
    Cut a BEAM release: bump updater.py's APP_VERSION, tag it, push the tag.

.DESCRIPTION
    Invoked as `git release`. The version constant and the tag are one contract --
    .github/workflows/release.yml refuses to build a tag whose name disagrees with
    APP_VERSION, and a shipped build compares that constant against the release tag
    to decide whether it is out of date. Bumping one by hand and forgetting the
    other is silent in the worst way: every client concludes it is up to date and
    the release reaches nobody. So this does both, in the one order that works.

    The bump is COMMITTED before the tag is created. A tag is a pointer at a
    commit, so tagging with the new APP_VERSION still sitting in the working tree
    would tag the OLD constant -- and CI, which checks out the tag, would fail the
    build it was asked to make. The commit is therefore not an extra courtesy; it
    is what makes the tag mean what it says.

    Only updater.py is committed. This repo routinely carries unrelated work in
    progress, and a release commit that swept it up would be both a surprise and
    impossible to revert cleanly.
#>

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

function Fail($message) {
    Write-Host "release: $message" -ForegroundColor Red
    exit 1
}

# Enter accepts the default, so a run with no console would silently take it and
# push a release nobody asked for. Refuse rather than guess.
if ([Console]::IsInputRedirected) {
    Fail "this must be run interactively -- it prompts for the version."
}

# --- Locate the repo and the constant -------------------------------------

$repo = (& git rev-parse --show-toplevel 2>$null)
if ($LASTEXITCODE -ne 0 -or -not $repo) { Fail "not inside a git repository." }
$repo = $repo.Trim()

$updaterPath = Join-Path $repo 'updater.py'
if (-not (Test-Path $updaterPath)) { Fail "updater.py not found at $updaterPath." }

$pattern = '(?m)^APP_VERSION\s*=\s*"([^"]+)"'
$content = [System.IO.File]::ReadAllText($updaterPath)
$match = [regex]::Match($content, $pattern)
if (-not $match.Success) { Fail "APP_VERSION not found in updater.py." }
$current = $match.Groups[1].Value

# --- Work out the default next version ------------------------------------
# Increment the last numeric component. A pre-release suffix is dropped rather
# than incremented: "0.2.0-rc1" defaults to "0.2.1", because there is no sensible
# way to guess whether the next one is rc2 or the real thing.

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
Write-Host "Current APP_VERSION: $current"
if ($default) {
    $answer = Read-Host "Version to release [$default]"
} else {
    Write-Host "(cannot derive a default from '$current' -- type the version in full)"
    $answer = Read-Host "Version to release"
}
if ([string]::IsNullOrWhiteSpace($answer)) { $answer = $default }
if ([string]::IsNullOrWhiteSpace($answer)) { Fail "no version given." }

# Accept a leading v so a typed "v0.2.0" does the obvious thing rather than
# producing a tag called "vv0.2.0".
$version = $answer.Trim()
$version = $version -replace '^[vV]', ''

if ($version -notmatch '^\d+(\.\d+)*(-[0-9A-Za-z.-]+)?$') {
    Fail "'$version' does not look like a version number."
}

$tag = "v$version"

# --- Conflict checks ------------------------------------------------------
# Local tags first, then the remote's, because a tag that exists only on origin
# still makes the push fail -- and it fails AFTER the commit has been made,
# which is the state worth not being in.

& git fetch --tags --quiet 2>$null | Out-Null

$existing = (& git tag --list $tag)
if ($existing) { Fail "tag $tag already exists locally. Nothing has been changed." }

$remote = (& git ls-remote --tags origin "refs/tags/$tag" 2>$null)
if ($LASTEXITCODE -eq 0 -and $remote) {
    Fail "tag $tag already exists on origin. Nothing has been changed."
}

if ($version -eq $current) {
    Fail "APP_VERSION is already $current and no tag exists for it. Pick a new version, or tag $tag by hand."
}

$branch = (& git rev-parse --abbrev-ref HEAD).Trim()
if ($branch -eq 'HEAD') { Fail "HEAD is detached; check out a branch first." }

# --- Write the constant ---------------------------------------------------

Write-Host ""
Write-Host "Releasing $tag (APP_VERSION $current -> $version) from branch $branch"

$updated = [regex]::Replace($content, $pattern, "APP_VERSION = `"$version`"", 1)
if ($updated -eq $content) { Fail "failed to rewrite APP_VERSION." }
# No BOM: updater.py is ASCII and Python reads it as UTF-8.
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($updaterPath, $updated, $utf8NoBom)
Write-Host "  updated updater.py"

# --- Commit, tag, push ----------------------------------------------------

& git add -- $updaterPath
if ($LASTEXITCODE -ne 0) { Fail "git add failed." }

& git commit -m "Release $tag" -- $updaterPath
if ($LASTEXITCODE -ne 0) { Fail "git commit failed. updater.py has been edited but nothing was tagged." }
Write-Host "  committed the version bump"

& git tag -a $tag -m "Release $tag"
if ($LASTEXITCODE -ne 0) { Fail "git tag failed. The bump is committed; tag it by hand." }
Write-Host "  created annotated tag $tag"

# The branch goes first. Pushing a tag alone uploads the objects it needs but
# leaves origin's branch behind it, so the release commit would exist on the
# remote with nothing pointing at it but the tag.
& git push origin $branch
if ($LASTEXITCODE -ne 0) { Fail "pushing $branch failed. The tag exists locally; push it once the branch is up." }

& git push origin $tag
if ($LASTEXITCODE -ne 0) { Fail "pushing $tag failed. Run: git push origin $tag" }

Write-Host ""
Write-Host "Pushed $tag. The release workflow builds and publishes it:" -ForegroundColor Green
Write-Host "  https://github.com/ironcross32/BeamNG-Accessibility-Mod/actions"
