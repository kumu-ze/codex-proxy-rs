use gateway_host::plugins::{PluginPackage, install_package};

#[test]
fn installation_copies_only_verified_files_and_preserves_existing_version() {
    let source = super::package(b"example");
    std::fs::write(source.path().join("unlisted-secret"), b"not part of plugin").unwrap();
    let destination = tempfile::tempdir().unwrap();
    let installed = install_package(source.path(), destination.path()).unwrap();
    assert_eq!(installed.manifest().version, "1.0.0");
    let path = destination.path().join("example-1.0.0");
    assert!(!path.join("unlisted-secret").exists());
    assert!(install_package(source.path(), destination.path()).is_err());
    assert!(PluginPackage::open(&path).is_ok());
}
