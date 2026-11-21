// Tauri build script. Selects the Windows application manifest: the default one
// (Common Controls v6 only, which Tauri's dialogs need) or, with the `uac-admin`
// feature, the same plus requestedExecutionLevel=requireAdministrator. Releases do not
// use it: the keyboard hook needs no elevation while the game runs un-elevated, which it
// does when launched through Steam. The feature is for a user who runs the game as
// administrator (see rust_plans/01-scope-and-architecture.md).

fn main() {
    println!("cargo:rerun-if-changed=manifests/default.xml");
    println!("cargo:rerun-if-changed=manifests/admin.xml");

    let admin = std::env::var_os("CARGO_FEATURE_UAC_ADMIN").is_some();
    let manifest = if admin {
        include_str!("manifests/admin.xml")
    } else {
        include_str!("manifests/default.xml")
    };
    // Test binaries get no manifest from tauri-build, so they load Common Controls v5,
    // which lacks TaskDialogIndirect, and die at load with STATUS_ENTRYPOINT_NOT_FOUND.
    // Embed the default manifest into them directly.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-tests=/MANIFESTINPUT:{dir}\\manifests\\default.xml");
    }

    let windows = tauri_build::WindowsAttributes::new().app_manifest(manifest);
    let attrs = tauri_build::Attributes::new().windows_attributes(windows);
    tauri_build::try_build(attrs).expect("failed to run the Tauri build script");
}
