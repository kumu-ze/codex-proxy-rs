mod install;
mod manifest;
#[cfg(unix)]
mod process;

fn package(script: &[u8]) -> tempfile::TempDir {
    use sha2::{Digest as _, Sha256};
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("worker"), script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            dir.path().join("worker"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    let manifest = serde_json::json!({"id":"example", "version":"1.0.0", "apiVersion":1,
        "executable":"worker", "files":{"worker":hex::encode(Sha256::digest(script))}});
    std::fs::write(
        dir.path().join("plugin.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    dir
}
