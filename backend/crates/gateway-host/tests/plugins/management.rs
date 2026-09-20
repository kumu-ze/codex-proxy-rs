use gateway_admin::ports::plugins::{PluginManagement, PluginOperations};
use gateway_host::plugins::{PluginConfig, PluginRegistry};
use serde_json::json;

const SCRIPT: &[u8] = br#"#!/usr/bin/python3
import sys,struct,json
while True:
 p=sys.stdin.buffer.read(4)
 if len(p)!=4: break
 r=json.loads(sys.stdin.buffer.read(struct.unpack('>I',p)[0]))
 b=json.dumps({'jsonrpc':'2.0','id':r['id'],'result':r['params']}).encode()
 sys.stdout.buffer.write(struct.pack('>I',len(b))+b);sys.stdout.buffer.flush()
"#;

fn archive(package: &std::path::Path, symlink: bool) -> Vec<u8> {
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(gzip);
    for name in ["plugin.json", "worker"] {
        tar.append_path_with_name(package.join(name), name).unwrap();
    }
    if symlink {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        tar.append_link(&mut header, "escape", "../../outside")
            .unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

async fn serve(bytes: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
    // 下载器拒绝回环地址；测试使用容器网卡地址，不向外部发送数据。
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    probe.connect("192.0.2.1:80").unwrap();
    let ip = probe.local_addr().unwrap().ip();
    let listener = tokio::net::TcpListener::bind((ip, 0)).await.unwrap();
    let url = format!("http://{}/package", listener.local_addr().unwrap());
    let app = axum::Router::new().route(
        "/package",
        axum::routing::get(move || {
            let bytes = bytes.clone();
            async move { bytes }
        }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, task)
}

#[tokio::test]
async fn url_install_enable_disable_restart_uninstall_and_reinstall_keep_data() {
    let package = super::package(SCRIPT);
    let root = tempfile::tempdir().unwrap();
    let (url, server) = serve(archive(package.path(), false)).await;
    let registry = PluginRegistry::start_managed(vec![], root.path())
        .await
        .unwrap();
    let install = PluginManagement::Install {
        url,
        sha256: None,
        proxy_id: None,
    };
    registry.manage(install.clone()).await.unwrap();
    assert!(!registry.list().await[0].enabled);
    assert!(registry.manage(install.clone()).await.is_err());
    registry
        .manage(PluginManagement::Enable {
            id: "example".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        registry
            .invoke("example", "admin.echo", json!({"ok":true}))
            .await
            .unwrap(),
        json!({"ok":true})
    );
    assert!(
        registry
            .manage(PluginManagement::Uninstall {
                id: "example".into()
            })
            .await
            .is_err()
    );
    std::fs::write(root.path().join("data/example/saved"), "retained").unwrap();
    registry
        .manage(PluginManagement::Disable {
            id: "example".into(),
        })
        .await
        .unwrap();
    assert!(!registry.list().await[0].available);
    assert!(
        registry
            .invoke("example", "admin.echo", json!({}))
            .await
            .is_err()
    );
    registry.shutdown().await;
    let registry = PluginRegistry::start_managed(vec![], root.path())
        .await
        .unwrap();
    assert!(!registry.list().await[0].enabled);
    registry
        .manage(PluginManagement::Uninstall {
            id: "example".into(),
        })
        .await
        .unwrap();
    assert!(registry.list().await.is_empty());
    registry.manage(install).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("data/example/saved")).unwrap(),
        "retained"
    );
    registry
        .manage(PluginManagement::Enable {
            id: "example".into(),
        })
        .await
        .unwrap();
    registry.shutdown().await;
    let registry = PluginRegistry::start_managed(vec![], root.path())
        .await
        .unwrap();
    assert!(registry.list().await[0].available);
    registry.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn invalid_downloads_leave_registry_and_existing_plugin_unchanged() {
    let package = super::package(SCRIPT);
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let registry = PluginRegistry::start_managed(
        vec![PluginConfig {
            directory: package.path().into(),
            data_directory: data.path().into(),
        }],
        root.path(),
    )
    .await
    .unwrap();
    for (bytes, sha256) in [
        (archive(package.path(), true), None),
        (archive(package.path(), false), Some("0".repeat(64))),
        (b"not an archive".to_vec(), None),
    ] {
        let (url, server) = serve(bytes).await;
        assert!(
            registry
                .manage(PluginManagement::Install {
                    url,
                    sha256,
                    proxy_id: None
                })
                .await
                .is_err()
        );
        server.abort();
        assert!(registry.list().await[0].available);
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".download-")
        }));
    }
    for url in [
        "file:///etc/passwd",
        "http://127.0.0.1/",
        "http://169.254.169.254/",
        "http://user:secret@example.com/",
    ] {
        assert!(
            registry
                .manage(PluginManagement::Install {
                    url: url.into(),
                    sha256: None,
                    proxy_id: None,
                })
                .await
                .is_err()
        );
    }
    registry.shutdown().await;
}

#[tokio::test]
async fn persistence_failure_does_not_disable_running_plugin() {
    let package = super::package(SCRIPT);
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let registry = PluginRegistry::start_managed(
        vec![PluginConfig {
            directory: package.path().into(),
            data_directory: data.path().into(),
        }],
        root.path(),
    )
    .await
    .unwrap();
    std::fs::rename(
        root.path().join("registry.json"),
        root.path().join("saved.json"),
    )
    .unwrap();
    std::fs::create_dir(root.path().join("registry.json")).unwrap();
    assert!(
        registry
            .manage(PluginManagement::Disable {
                id: "example".into()
            })
            .await
            .is_err()
    );
    assert!(registry.list().await[0].available);
    assert!(registry.list().await[0].enabled);
    registry.shutdown().await;
}

#[tokio::test]
async fn disabled_extension_is_skipped_but_enabled_failure_is_not_bypassed() {
    use gateway_core::engine::extensions::{ExtensionRequest, RequestExtension};
    let package = super::package(SCRIPT);
    let manifest_path = package.path().join("plugin.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["capabilities"] = json!(["request.openai"]);
    std::fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let registry = PluginRegistry::start_managed(
        vec![PluginConfig {
            directory: package.path().into(),
            data_directory: data.path().into(),
        }],
        root.path(),
    )
    .await
    .unwrap();
    let request = ExtensionRequest {
        provider: "openai".into(),
        account_id: "one".into(),
        model: "test".into(),
        credential_scope: "scope".into(),
        authentication_kind: "oauth".into(),
        plan_type: None,
        account_eligible: true,
    };
    assert!(registry.before_send(request.clone()).await.is_err());
    registry
        .manage(PluginManagement::Disable {
            id: "example".into(),
        })
        .await
        .unwrap();
    assert!(!registry.before_send(request).await.unwrap().deny);
    registry.shutdown().await;
}
#[tokio::test]
async fn url_install_rejects_overlap_with_imported_data_directory() {
    let imported = super::package(SCRIPT);
    let manifest_path = imported.path().join("plugin.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["id"] = json!("legacy");
    std::fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let registry = PluginRegistry::start_managed(
        vec![PluginConfig {
            directory: imported.path().into(),
            data_directory: root.path().join("data"),
        }],
        root.path(),
    )
    .await
    .unwrap();
    let package = super::package(SCRIPT);
    let (url, server) = serve(archive(package.path(), false)).await;
    assert!(
        registry
            .manage(PluginManagement::Install {
                url,
                sha256: None,
                proxy_id: None
            })
            .await
            .is_err()
    );
    assert_eq!(registry.list().await.len(), 1);
    assert!(registry.list().await[0].available);
    assert!(!root.path().join("packages/example-1.0.0").exists());
    registry.shutdown().await;
    server.abort();
}
#[tokio::test]
async fn installation_uses_selected_proxy_and_reports_download_failure_types() {
    use gateway_admin::ports::plugins::PluginOperationError;
    use gateway_core::engine::extensions::{ExtensionServices, ExtensionUnavailable};
    use std::sync::Arc;
    struct ProxyService(String);
    #[async_trait::async_trait]
    impl ExtensionServices for ProxyService {
        async fn invoke(
            &self,
            method: &str,
            input: serde_json::Value,
        ) -> Result<serde_json::Value, ExtensionUnavailable> {
            assert_eq!(method, "proxies.resolve");
            assert_eq!(input["id"], "chosen");
            Ok(json!(self.0))
        }
    }
    let package = super::package(SCRIPT);
    let root = tempfile::tempdir().unwrap();
    let (url, server) = serve(archive(package.path(), false)).await;
    let proxy = url.trim_end_matches("/package").to_owned();
    let mut destination = reqwest::Url::parse(&url).unwrap();
    destination.set_port(Some(1)).unwrap();
    let registry = PluginRegistry::start_managed(vec![], root.path())
        .await
        .unwrap();
    registry
        .attach_services(Arc::new(ProxyService(proxy)))
        .unwrap();
    let operation = PluginManagement::Install {
        url: destination.to_string(),
        sha256: None,
        proxy_id: Some("chosen".into()),
    };
    registry.manage(operation).await.unwrap();
    assert_eq!(registry.list().await[0].id, "example");
    assert!(!registry.list().await[0].enabled);
    let error = registry
        .manage(PluginManagement::Install {
            url,
            sha256: Some("0".repeat(64)),
            proxy_id: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(error, PluginOperationError::Checksum));
    let error = registry
        .manage(PluginManagement::Install {
            url: destination.to_string(),
            sha256: None,
            proxy_id: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        PluginOperationError::Download | PluginOperationError::DownloadTimeout
    ));
    registry.shutdown().await;
    server.abort();
}
