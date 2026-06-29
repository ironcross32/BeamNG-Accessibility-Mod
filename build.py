"""
Build script for BEAM.
Usage:
    uv run python build.py          # build beamtel + package mod
    uv run python build.py --mod    # package mod only (skip nuitka)
    uv run python build.py --exe    # build beamtel only (skip mod)

Portability notes:
  * Output goes to ``build/`` inside the project (created if missing) — there is
    no dependency on any machine-specific folder.
  * ``bng_mod`` is expected to be a directory junction pointing at BeamNG's
    standard unpacked-mods location. If it is missing the script creates the
    junction automatically (see ``ensure_mod_junction``).
"""

import argparse
import datetime
import os
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

PROJECT_DIR = Path(__file__).parent.resolve()
# Nuitka writes beamtel.exe here (see --output-dir in beamtel.py's pragmas), and
# this is also where we stage everything that gets bundled into the release zip.
BUILD_DIR = PROJECT_DIR / "build"
OUTPUT_DIR = BUILD_DIR
MOD_JUNCTION = PROJECT_DIR / "bng_mod"

# Data files Nuitka pulls in via --include-data-file (paths are relative to the
# project dir at build time). SRAL.dll / nvdaControllerClient.dll are gitignored,
# so a fresh clone will not have them — fail early with a clear message instead of
# letting Nuitka die deep into a build.
REQUIRED_BUILD_INPUTS = (
    "SRAL.dll",
    "nvdaControllerClient.dll",
    "hrtf_kemar_horizontal.npz",
)

# Nuitka scratch directories that may appear in BUILD_DIR; never bundle these, and
# never let them leak into the release zip. ".appdata" is the onefile runtime
# unpack folder created when the exe is *run* from BUILD_DIR.
SCRATCH_SUFFIXES = (".dist", ".build", ".onefile-build")
SCRATCH_NAMES = {".appdata", "__pycache__"}


def beamng_mod_target() -> Path:
    """Standard per-user location of the unpacked screen-reader mod.

    %LOCALAPPDATA%\\BeamNG\\BeamNG.drive\\current\\mods\\unpacked\\bng_screenreader_mod
    'current' is a stable junction BeamNG maintains pointing at the active version,
    so this path is the same on every machine.
    """
    local = os.environ.get("LOCALAPPDATA") or os.path.join(
        os.path.expanduser("~"), "AppData", "Local"
    )
    return (
        Path(local)
        / "BeamNG"
        / "BeamNG.drive"
        / "current"
        / "mods"
        / "unpacked"
        / "bng_screenreader_mod"
    )


def ensure_mod_junction() -> bool:
    """Make sure ``bng_mod`` resolves to BeamNG's mods folder.

    - If it already exists (a junction, or a real folder materialized by git),
      leave it alone and just report where it points.
    - If a broken junction is present, remove it and recreate.
    - If it is missing entirely, create a junction to the standard location
      (provided that location exists).
    """
    target = beamng_mod_target()

    if os.path.exists(MOD_JUNCTION):
        resolved = Path(os.path.realpath(MOD_JUNCTION))
        print(f"Mod junction OK: {MOD_JUNCTION.name} -> {resolved}")
        if resolved != target and resolved != target.resolve():
            print(
                "  NOTE: it does not point at the standard BeamNG location\n"
                f"        ({target}). Leaving it as-is."
            )
        return True

    # Path may be a dangling junction (link present, target gone): exists() is
    # False but lexists() is True. Clear it so mklink can recreate it.
    if os.path.lexists(MOD_JUNCTION):
        print(f"Removing dangling junction at {MOD_JUNCTION}")
        try:
            os.rmdir(MOD_JUNCTION)
        except OSError:
            try:
                os.remove(MOD_JUNCTION)
            except OSError as e:
                print(f"ERROR: could not remove existing {MOD_JUNCTION}: {e}")
                return False

    if not target.exists():
        print(
            "ERROR: cannot create the mod junction because BeamNG's mod folder\n"
            f"       was not found at:\n         {target}\n"
            "       Install/spawn the mod in BeamNG once (so the folder exists),\n"
            "       or create the 'bng_mod' junction manually, then re-run."
        )
        return False

    print(f"Creating junction: {MOD_JUNCTION} -> {target}")
    result = subprocess.run(
        ["cmd", "/c", "mklink", "/J", str(MOD_JUNCTION), str(target)],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(f"ERROR: mklink failed: {result.stdout.strip()} {result.stderr.strip()}")
        return False
    return True


def check_build_inputs() -> bool:
    """Verify the data files Nuitka needs are present before kicking off a build."""
    missing = [name for name in REQUIRED_BUILD_INPUTS if not (PROJECT_DIR / name).exists()]
    if missing:
        print("ERROR: required build input(s) missing from the project directory:")
        for name in missing:
            print(f"  - {name}")
        print(
            "\nNote: the .dll files are gitignored and are not part of a fresh\n"
            "clone — copy them into the project root before building."
        )
        return False
    return True


def _clean_scratch() -> None:
    """Remove Nuitka intermediate directories from BUILD_DIR (in case --remove-output
    left anything behind), so they never end up in the release zip."""
    if not BUILD_DIR.exists():
        return
    for child in BUILD_DIR.iterdir():
        if child.is_dir() and (
            child.name in SCRATCH_NAMES or child.name.endswith(SCRATCH_SUFFIXES)
        ):
            shutil.rmtree(child, ignore_errors=True)


def build_exe() -> bool:
    print("=== Building beamtel.exe with Nuitka ===")
    if not check_build_inputs():
        return False

    result = subprocess.run(
        ["uv", "run", "nuitka", "beamtel.py"],
        cwd=PROJECT_DIR,
    )
    if result.returncode != 0:
        print(f"\nERROR: Nuitka build failed (exit code {result.returncode})")
        return False

    exe = BUILD_DIR / "beamtel.exe"
    if not exe.exists():
        print(f"ERROR: Build succeeded but {exe} not found")
        return False

    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    dest = OUTPUT_DIR / "beamtel.exe"
    # Nuitka already writes into BUILD_DIR, which is OUTPUT_DIR, so the copy is a
    # no-op there. Only copy when the two locations actually differ.
    if exe.resolve() != dest.resolve():
        shutil.copy2(exe, dest)
        print(f"Copied {exe.name} -> {dest}")
    else:
        print(f"beamtel.exe already in {OUTPUT_DIR}")

    readme = PROJECT_DIR / "README.md"
    if readme.exists():
        readme_dest = OUTPUT_DIR / "README.md"
        shutil.copy2(readme, readme_dest)
        print(f"Copied {readme.name} -> {readme_dest}")

    _clean_scratch()
    return True


def package_mod() -> bool:
    print("=== Packaging mod ===")

    # Resolve the junction (or real folder) to the actual mod directory.
    mod_real = Path(os.path.realpath(MOD_JUNCTION))
    if not mod_real.exists():
        print(f"ERROR: Mod directory not found at {mod_real}")
        return False

    mod_name = mod_real.name
    zip_name = mod_name + ".zip"
    zip_path = OUTPUT_DIR / zip_name

    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

    file_count = 0
    with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for file in sorted(mod_real.rglob("*")):
            if file.is_file():
                arcname = file.relative_to(mod_real)
                zf.write(file, arcname)
                file_count += 1
                print(f"  + {arcname}")

    print(f"\nPackaged {file_count} files -> {zip_path}")
    return True


def _is_scratch(rel: Path) -> bool:
    """True if a path under OUTPUT_DIR belongs to Nuitka scratch / runtime unpack."""
    for part in rel.parts:
        if part in SCRATCH_NAMES or part.endswith(SCRATCH_SUFFIXES):
            return True
    return False


def bundle_output() -> None:
    print("=== Bundling output directory ===")
    now = datetime.datetime.now()
    zip_name = now.strftime("BeamNG_accessibility_mod_%Y-%m-%d_%H-%M.zip")
    zip_path = OUTPUT_DIR / zip_name

    for existing in OUTPUT_DIR.glob("BeamNG_accessibility_mod_*.zip"):
        existing.unlink()
        print(f"Removed existing {existing.name}")

    file_count = 0
    with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for file in sorted(OUTPUT_DIR.rglob("*")):
            if not file.is_file() or file == zip_path:
                continue
            arcname = file.relative_to(OUTPUT_DIR)
            if _is_scratch(arcname):
                continue
            zf.write(file, arcname)
            file_count += 1
            print(f"  + {arcname}")

    print(f"Bundled {file_count} files -> {zip_path}")


def main():
    parser = argparse.ArgumentParser(description="BEAM build script")
    parser.add_argument("--mod", action="store_true", help="Package mod only")
    parser.add_argument("--exe", action="store_true", help="Build exe only")
    args = parser.parse_args()

    do_exe = not args.mod
    do_mod = not args.exe

    # The mod junction is needed for packaging the mod; ensure it up front so the
    # build fails fast (and with a useful message) on a machine that lacks it.
    if do_mod:
        if not ensure_mod_junction():
            print("\nCannot proceed without the mod junction.")
            sys.exit(1)

    success = True

    if do_exe:
        if not build_exe():
            success = False
            if do_mod:
                print("Skipping mod packaging due to build failure.")
                sys.exit(1)

    if do_mod:
        if not package_mod():
            success = False

    if success:
        bundle_output()
        print("\nBuild complete.")
    else:
        print("\nBuild finished with errors.")
        sys.exit(1)


if __name__ == "__main__":
    main()
