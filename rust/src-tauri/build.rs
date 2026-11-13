// Tauri build script. Selects the Windows application manifest: the default one
// (Common Controls v6 only, which Tauri's dialogs need) or, with the `uac-admin`
// feature, the same plus requestedExecutionLevel=requireAdministrator, because the
// keyboard hooks must work while the elevated game has focus.

fn main() {
    println!("cargo:rerun-if-changed=manifests/default.xml");
    println!("cargo:rerun-if-changed=manifests/admin.xml");

    let admin = std::env::var_os("CARGO_FEATURE_UAC_ADMIN").is_some();
    let manifest = if admin {
        include_str!("manifests/admin.xml")
    } else {
        include_str!("manifests/default.xml")
    };
    let windows = tauri_build::WindowsAttributes::new().app_manifest(manifest);
    let attrs = tauri_build::Attributes::new().windows_attributes(windows);
    tauri_build::try_build(attrs).expect("failed to run the Tauri build script");
}
