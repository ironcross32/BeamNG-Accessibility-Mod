"""
Build script for BEAM.
Usage:
    uv run python build.py          # build beamtel + package mod
    uv run python build.py --mod    # package mod only (skip nuitka)
    uv run python build.py --exe    # build beamtel only (skip mod)
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
OUTPUT_DIR = Path(r"C:\programming\python\beam_complete")
BUILD_DIR = PROJECT_DIR / "build"
MOD_JUNCTION = PROJECT_DIR / "bng_mod"


def build_exe() -> bool:
    print("=== Building beamtel.exe with Nuitka ===")
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
    shutil.copy2(exe, dest)
    print(f"Copied {exe.name} -> {dest}")

    readme = PROJECT_DIR / "README.md"
    if readme.exists():
        readme_dest = OUTPUT_DIR / "README.md"
        shutil.copy2(readme, readme_dest)
        print(f"Copied {readme.name} -> {readme_dest}")

    return True


def package_mod() -> bool:
    print("=== Packaging mod ===")

    # Resolve the junction to the real mod directory
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


def main():
    parser = argparse.ArgumentParser(description="BEAM build script")
    parser.add_argument("--mod", action="store_true", help="Package mod only")
    parser.add_argument("--exe", action="store_true", help="Build exe only")
    args = parser.parse_args()

    do_exe = not args.mod
    do_mod = not args.exe

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
            if file.is_file() and file != zip_path:
                arcname = file.relative_to(OUTPUT_DIR)
                zf.write(file, arcname)
                file_count += 1
                print(f"  + {arcname}")

    print(f"Bundled {file_count} files -> {zip_path}")


if __name__ == "__main__":
    main()
