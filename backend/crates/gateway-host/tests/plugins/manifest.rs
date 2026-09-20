use gateway_host::plugins::{PluginError, PluginPackage};

#[test]
fn rejects_tampering_and_unknown_api() {
    let dir = super::package(b"example");
    assert_eq!(
        PluginPackage::open(dir.path()).unwrap().manifest().id,
        "example"
    );
    std::fs::write(dir.path().join("worker"), b"modified").unwrap();
    assert!(matches!(
        PluginPackage::open(dir.path()),
        Err(PluginError::InvalidPackage)
    ));
    let path = dir.path().join("plugin.json");
    let data = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\"apiVersion\":1", "\"apiVersion\":2");
    std::fs::write(path, data).unwrap();
    assert!(matches!(
        PluginPackage::open(dir.path()),
        Err(PluginError::Incompatible)
    ));
}

#[test]
fn rejects_parent_paths_and_windows_paths_on_every_platform() {
    for name in ["../worker", "/worker", "C:/worker", "sub\\worker"] {
        let dir = super::package(b"example");
        let path = dir.path().join("plugin.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let digest = manifest["files"]["worker"].take();
        manifest["executable"] = name.into();
        manifest["files"] = serde_json::json!({name:digest});
        std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(matches!(
            PluginPackage::open(dir.path()),
            Err(PluginError::InvalidPackage)
        ));
    }
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_executables() {
    let dir = super::package(b"example");
    std::fs::rename(dir.path().join("worker"), dir.path().join("real")).unwrap();
    std::os::unix::fs::symlink("real", dir.path().join("worker")).unwrap();
    assert!(matches!(
        PluginPackage::open(dir.path()),
        Err(PluginError::InvalidPackage)
    ));
}
#[test]
fn chat_ui_capability_is_explicit_and_unknown_capabilities_are_rejected() {
    for (capabilities, allowed) in [
        (serde_json::json!(["ui.chat"]), true),
        (
            serde_json::json!(["request.openai", "provider.openai", "ui.chat"]),
            true,
        ),
        (serde_json::json!(["ui.admin"]), false),
    ] {
        let dir = super::package(b"example");
        let path = dir.path().join("plugin.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["capabilities"] = capabilities;
        std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert_eq!(PluginPackage::open(dir.path()).is_ok(), allowed);
    }
}

#[test]
fn validates_display_metadata_without_enabling_unsafe_links() {
    for (repository, author, asset, accepted) in [
        (
            "https://github.com/owner/plugin",
            "Author",
            "plugin-{version}-linux-x64.tar.gz",
            true,
        ),
        (
            "javascript:alert(1)",
            "Author",
            "plugin-{version}.tar.gz",
            false,
        ),
        (
            "https://user:password@github.com/owner/plugin",
            "Author",
            "plugin-{version}.tar.gz",
            false,
        ),
        (
            "https://github.com/owner/plugin",
            "",
            "plugin-{version}.tar.gz",
            false,
        ),
        (
            "https://github.com/owner/plugin",
            "Author",
            "../plugin-{version}.tar.gz",
            false,
        ),
    ] {
        let dir = super::package(b"worker");
        let path = dir.path().join("plugin.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["repository"] = serde_json::json!(repository);
        manifest["author"] = serde_json::json!(author);
        manifest["releaseAsset"] = serde_json::json!(asset);
        std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert_eq!(PluginPackage::open(dir.path()).is_ok(), accepted);
    }
}
