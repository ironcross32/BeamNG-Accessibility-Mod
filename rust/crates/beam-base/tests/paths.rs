//! beam_base::paths against the real checkout and a scratch "install" directory.

use beam_base::paths::*;
use beam_testutil::repo_root;

#[test]
fn repo_root_is_found_from_inside_the_workspace() {
    let here = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let found = find_repo_root_from(&here).expect("repo root");
    assert_eq!(
        found.canonicalize().unwrap(),
        repo_root().canonicalize().unwrap()
    );
    // The test binary runs from rust/target/<profile>/deps, inside the checkout.
    assert!(beam_base::paths::repo_root().is_some());
}

#[test]
fn data_file_beside_the_exe_wins_over_the_checkout() {
    let install = tempfile::tempdir().unwrap();
    let repo = repo_root();
    let name = "hrtf_mit_kemar.npz";

    let from_repo = find_data_file_in(name, install.path(), Some(&repo)).expect("in repo");
    assert_eq!(from_repo, repo.join(name));

    std::fs::write(install.path().join(name), b"x").unwrap();
    let beside = find_data_file_in(name, install.path(), Some(&repo)).unwrap();
    assert_eq!(beside, install.path().join(name));

    assert_eq!(
        find_data_file_in("no_such_file.npz", install.path(), Some(&repo)),
        None
    );
    assert_eq!(
        find_data_file_in("no_such_file.npz", install.path(), None),
        None
    );
}

#[test]
fn hrtf_sets_are_all_found_from_a_dev_build() {
    for info in beam_base::hrtf_sets::available_sets() {
        assert!(find_data_file(info.filename).is_some(), "{}", info.filename);
    }
    assert!(hrtf_base_dir().join("hrtf_mit_kemar.npz").is_file());
}
