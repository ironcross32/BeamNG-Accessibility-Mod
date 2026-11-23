//! Build and package BEAM: the port of build.py (workstream R1).
//!
//! Run as `cargo xtask <command>` (the alias is in `.cargo/config.toml`).
//!
//! The release is ONE asset, `BeamNG_accessibility_mod.zip`, and its layout is a contract
//! with `beam-engine`'s updater, which validates it, unpacks it into a staging directory
//! and robocopies that over the user's program directory. So everything the exe reads
//! from beside itself must be in it: the HRTF sets (`paths::find_data_file`), their
//! licence notice (the KU100 set's CC BY-SA licence requires the attribution to travel
//! with it) and the mod zip the installer copies into BeamNG.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use zip::write::SimpleFileOptions;

/// The application version: the workspace's `[workspace.package] version`, which is also
/// `beam_engine::updater::APP_VERSION` (both are `CARGO_PKG_VERSION`). A shipped build
/// compares it against the latest release tag, so the tag must equal it with the "v"
/// stripped; `check-tag` enforces that in the release workflow.
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `updater::ASSET_NAME`. Fixed, with no version or date in it, so that
/// `<repo>/releases/latest/download/BeamNG_accessibility_mod.zip` is a permanent link.
const BUNDLE_NAME: &str = "BeamNG_accessibility_mod.zip";

/// `mod_install::MOD_ZIP`, and the folder name BeamNG gives the mod. A constant rather
/// than the mod directory's name: locally `bng_mod` is a junction whose target may be
/// called anything, and on a CI checkout it is a real directory called `bng_mod`.
const MOD_ZIP_NAME: &str = "bng_screenreader_mod.zip";

/// `mod_install::MOD_VERSION_MEMBER`. Stamped into the mod zip and read back by the
/// installer, because file times are not versions: copying preserves them from another
/// machine's clock and extraction invents "now".
const MOD_VERSION_MEMBER: &str = "bnvda_mod_version.json";

/// The exe as cargo names it (`mainBinaryName` in tauri.conf.json, `[[bin]]` in
/// src-tauri/Cargo.toml).
const EXE_NAME: &str = "beamtel.exe";

/// Repository files copied beside the exe as they are.
const HRTF_NOTICES: &str = "HRTF_DATA_LICENSES.md";
const DOCS: [&str; 2] = ["README.md", HRTF_NOTICES];

/// `updater::REQUIRED_MEMBERS`: a bundle without these is refused by every client.
const REQUIRED_MEMBERS: [&str; 2] = [EXE_NAME, MOD_ZIP_NAME];

type Result<T, E = String> = std::result::Result<T, E>;

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  dist [--no-build]   build the release exe, package the mod, and write
                      target/dist/BeamNG_accessibility_mod.zip; the unzipped
                      layout is left in target/dist/BeamNG_accessibility_mod/,
                      runnable as it is. --no-build packages the exe already in
                      target/release instead of building it.
  mod                 package the mod alone: target/dist/bng_screenreader_mod.zip
  version             print the application version
  check-tag <tag>     fail unless <tag> is v<version> (the release workflow's check)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        [] | ["help" | "--help" | "-h"] => {
            println!("{USAGE}");
            Ok(())
        }
        ["dist"] => dist(true),
        ["dist", "--no-build"] => dist(false),
        ["mod"] => package_mod_only(),
        ["version"] => {
            println!("{APP_VERSION}");
            Ok(())
        }
        ["check-tag", tag] => check_tag(tag),
        _ => Err(format!("unknown command {:?}\n\n{USAGE}", args.join(" "))),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

// =========================
//  Paths
// =========================

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the workspace root")
        .to_path_buf()
}

fn target_dir(root: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => root.join(dir),
        None => root.join("target"),
    }
}

fn dist_dir(root: &Path) -> PathBuf {
    target_dir(root).join("dist")
}

// =========================
//  Commands
// =========================

fn check_tag(tag: &str) -> Result<()> {
    let tag_version = tag.strip_prefix("refs/tags/").unwrap_or(tag);
    let tag_version = tag_version.strip_prefix('v').unwrap_or(tag_version);
    if tag_version != APP_VERSION {
        return Err(format!(
            "the tag is {tag} but the workspace version is {APP_VERSION}. \
             Bump [workspace.package] version in Cargo.toml (git release does it) and re-tag."
        ));
    }
    println!("version {APP_VERSION} matches tag {tag}");
    Ok(())
}

fn package_mod_only() -> Result<()> {
    let root = workspace_root();
    let out = dist_dir(&root);
    fs::create_dir_all(&out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let stamp = Stamp::now();
    package_mod(&root, &out.join(MOD_ZIP_NAME), &stamp)
}

fn dist(build: bool) -> Result<()> {
    let root = workspace_root();
    let stamp = Stamp::now();

    // Check the inputs before spending two minutes on a release build.
    let mut copies: Vec<PathBuf> = Vec::new();
    for set in beam_base::hrtf_sets::BUNDLED_SETS {
        copies.push(root.join(set.filename));
    }
    for doc in DOCS {
        copies.push(root.join(doc));
    }
    let missing: Vec<String> = copies
        .iter()
        .filter(|p| !p.is_file())
        .map(|p| p.display().to_string())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "required file(s) missing:\n  {}\n(the hrtf_*.npz sets are committed; a missing \
             one must be baked from its .sofa outside the repository)",
            missing.join("\n  ")
        ));
    }

    if build {
        build_exe(&root)?;
    }
    let exe = target_dir(&root).join("release").join(EXE_NAME);
    if !exe.is_file() {
        return Err(format!(
            "{} not found; run without --no-build",
            exe.display()
        ));
    }
    copies.insert(0, exe);

    let dist = dist_dir(&root);
    let stage = dist.join(BUNDLE_NAME.trim_end_matches(".zip"));
    // Under target/, so ours to clear: a file left from an older layout would otherwise
    // ride along into the bundle and onto every user's machine.
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|e| format!("cannot clear {}: {e}", stage.display()))?;
    }
    fs::create_dir_all(&stage).map_err(|e| format!("cannot create {}: {e}", stage.display()))?;

    println!("=== Staging {} ===", stage.display());
    for src in &copies {
        let name = src.file_name().expect("a file path");
        fs::copy(src, stage.join(name))
            .map_err(|e| format!("cannot copy {}: {e}", src.display()))?;
        println!("  + {}", name.to_string_lossy());
    }
    package_mod(&root, &stage.join(MOD_ZIP_NAME), &stamp)?;

    let bundle = dist.join(BUNDLE_NAME);
    println!("=== Bundling {} ===", bundle.display());
    let mut members: Vec<(String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&stage).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let name = path
            .file_name()
            .expect("a file")
            .to_string_lossy()
            .into_owned();
        members.push((name, path));
    }
    members.sort();
    write_zip(&bundle, &members, &[], &stamp)?;
    verify_bundle(&bundle)?;

    let size = fs::metadata(&bundle).map(|m| m.len()).unwrap_or(0);
    println!(
        "\nBEAM {APP_VERSION}: {} ({:.1} MB, {} files)",
        bundle.display(),
        size as f64 / 1_048_576.0,
        members.len()
    );
    Ok(())
}

fn build_exe(root: &Path) -> Result<()> {
    println!("=== Building {EXE_NAME} (release) ===");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    // --locked: a release must be built from the Cargo.lock that was committed, never one
    // cargo quietly re-resolved on the build machine.
    let status = Command::new(cargo)
        .current_dir(root)
        .args(["build", "--release", "--locked", "-p", "beam-app"])
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("the release build failed ({status})"));
    }
    Ok(())
}

// =========================
//  The mod zip
// =========================

/// Zip `bng_mod/` (through the junction locally; a plain directory on a CI checkout) with
/// a fresh version stamp, paths relative to the mod root as BeamNG expects.
fn package_mod(root: &Path, zip_path: &Path, stamp: &Stamp) -> Result<()> {
    println!("=== Packaging the mod ===");
    let junction = root.join("bng_mod");
    let mod_dir = fs::canonicalize(&junction).map_err(|e| {
        format!(
            "the mod directory {} cannot be resolved ({e}). It is a junction into \
             BeamNG's unpacked mods folder on a development machine.",
            junction.display()
        )
    })?;

    let mut members: Vec<(String, PathBuf)> = Vec::new();
    collect_files(&mod_dir, &mod_dir, &mut members)?;
    // A stamp left in the live folder by something else must not shadow the fresh one.
    members.retain(|(name, _)| name != MOD_VERSION_MEMBER);
    members.sort();
    if members.is_empty() {
        return Err(format!("{} is empty", mod_dir.display()));
    }

    let stamp_json = serde_json::to_string_pretty(&serde_json::json!({
        "version": APP_VERSION,
        "built_utc": stamp.iso,
    }))
    .expect("a JSON object serialises");
    write_zip(
        zip_path,
        &members,
        &[(MOD_VERSION_MEMBER, stamp_json.as_bytes())],
        stamp,
    )?;
    println!(
        "Packaged {} files and the {APP_VERSION} stamp -> {}",
        members.len(),
        zip_path.display()
    );
    Ok(())
}

/// Every file under `dir`, as (forward-slash path relative to `base`, path).
fn collect_files(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            collect_files(base, &path, out)?;
        } else if kind.is_file() {
            let rel = path.strip_prefix(base).expect("under the base directory");
            let name = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push((name, path));
        }
    }
    Ok(())
}

// =========================
//  Zip writing
// =========================

/// Write `files` (read from disk) then `extra` (in memory) to a new deflated zip, every
/// member dated `stamp`.
fn write_zip(
    zip_path: &Path,
    files: &[(String, PathBuf)],
    extra: &[(&str, &[u8])],
    stamp: &Stamp,
) -> Result<()> {
    let file = fs::File::create(zip_path)
        .map_err(|e| format!("cannot create {}: {e}", zip_path.display()))?;
    let mut zw = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(stamp.zip_time);
    let mut buf = Vec::new();
    for (name, path) in files {
        buf.clear();
        fs::File::open(path)
            .and_then(|mut f| f.read_to_end(&mut buf))
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        zw.start_file(name.as_str(), options)
            .and_then(|()| zw.write_all(&buf).map_err(Into::into))
            .map_err(|e| format!("cannot write {name} to {}: {e}", zip_path.display()))?;
    }
    for (name, bytes) in extra {
        zw.start_file(*name, options)
            .and_then(|()| zw.write_all(bytes).map_err(Into::into))
            .map_err(|e| format!("cannot write {name} to {}: {e}", zip_path.display()))?;
    }
    zw.finish()
        .map_err(|e| format!("cannot finish {}: {e}", zip_path.display()))?;
    Ok(())
}

/// The updater's own acceptance test, run on what we are about to ship: every member reads
/// back, the required members are there, and so is everything the exe loads from beside
/// itself.
fn verify_bundle(bundle: &Path) -> Result<()> {
    let file = fs::File::open(bundle).map_err(|e| e.to_string())?;
    let mut zf = zip::ZipArchive::new(file).map_err(|e| format!("bundle unreadable: {e}"))?;
    let mut names = Vec::new();
    for i in 0..zf.len() {
        let mut member = zf
            .by_index(i)
            .map_err(|e| format!("bundle entry {i}: {e}"))?;
        let name = member
            .name()
            .map_err(|e| format!("bundle entry {i}: {e}"))?
            .into_owned();
        std::io::copy(&mut member, &mut std::io::sink())
            .map_err(|e| format!("bundle entry {name}: {e}"))?;
        names.push(name);
    }
    let wanted = REQUIRED_MEMBERS
        .iter()
        .copied()
        .chain(
            beam_base::hrtf_sets::BUNDLED_SETS
                .iter()
                .map(|s| s.filename),
        )
        .chain([HRTF_NOTICES]);
    for want in wanted {
        if !names.iter().any(|n| n == want) {
            return Err(format!("the bundle is missing {want}"));
        }
    }
    Ok(())
}

// =========================
//  The build time
// =========================

/// One instant for the whole build: the stamp's `built_utc` and every zip member's time.
struct Stamp {
    /// Python's `datetime.now(timezone.utc).replace(microsecond=0).isoformat()`.
    iso: String,
    zip_time: zip::DateTime,
}

impl Stamp {
    fn now() -> Stamp {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Stamp::at(secs)
    }

    fn at(unix_secs: u64) -> Stamp {
        let (y, mo, d) = civil_from_days((unix_secs / 86_400) as i64);
        let tod = unix_secs % 86_400;
        let (h, mi, s) = (
            (tod / 3600) as u8,
            (tod % 3600 / 60) as u8,
            (tod % 60) as u8,
        );
        Stamp {
            iso: format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}+00:00"),
            // A zip time cannot be before 1980; a clock that wrong gets the zip epoch.
            zip_time: zip::DateTime::from_date_and_time(y as u16, mo, d, h, mi, s)
                .unwrap_or_default(),
        }
    }
}

/// Days since 1970-01-01 to a proleptic Gregorian (year, month, day): Howard Hinnant's
/// `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        assert_eq!(civil_from_days(20_779), (2026, 11, 22));
    }

    #[test]
    fn stamp_formats_like_python_isoformat() {
        // 2026-11-22 13:04:05 UTC
        let s = Stamp::at(20_779 * 86_400 + 13 * 3600 + 4 * 60 + 5);
        assert_eq!(s.iso, "2026-11-22T13:04:05+00:00");
        assert_eq!(s.zip_time.year(), 2026);
        assert_eq!(s.zip_time.month(), 11);
    }

    #[test]
    fn tags() {
        assert!(check_tag(&format!("v{APP_VERSION}")).is_ok());
        assert!(check_tag(&format!("refs/tags/v{APP_VERSION}")).is_ok());
        assert!(check_tag("v0.0.0-not-this").is_err());
    }

    #[test]
    fn contract_with_the_updater() {
        // The updater's constants live in beam-engine, which xtask does not build (it
        // compiles Prism); the e10 updater test pins the same strings from that side.
        assert_eq!(BUNDLE_NAME, "BeamNG_accessibility_mod.zip");
        assert_eq!(
            REQUIRED_MEMBERS,
            ["beamtel.exe", "bng_screenreader_mod.zip"]
        );
        assert_eq!(MOD_VERSION_MEMBER, "bnvda_mod_version.json");
    }
}
