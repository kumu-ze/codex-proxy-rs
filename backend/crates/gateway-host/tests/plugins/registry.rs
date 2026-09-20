use gateway_admin::ports::plugins::PluginOperations;
use gateway_host::plugins::{PluginConfig, PluginRegistry};
use serde_json::json;
use std::time::Duration;

const SCRIPT: &[u8] = br#"#!/usr/bin/python3
import sys, struct, json, time
while True:
    prefix = sys.stdin.buffer.read(4)
    if len(prefix) != 4: break
    request = json.loads(sys.stdin.buffer.read(struct.unpack('>I',prefix)[0]))
    if request['method'] == 'admin.reject':
        body = json.dumps({'jsonrpc':'2.0','id':request['id'],'error':{'code':-32601,'message':'private diagnostic'}}).encode()
        sys.stdout.buffer.write(struct.pack('>I',len(body))+body); sys.stdout.buffer.flush(); continue
    if request['method'] == 'admin.sleep': time.sleep(2)
    result = request['params']
    body = json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}).encode()
    sys.stdout.buffer.write(struct.pack('>I',len(body))+body); sys.stdout.buffer.flush()
    if request['method'] == 'admin.exit': break
"#;

async fn registry() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    std::sync::Arc<PluginRegistry>,
) {
    let package = super::package(SCRIPT);
    let data = tempfile::tempdir().unwrap();
    let registry = PluginRegistry::start(vec![PluginConfig {
        directory: package.path().into(),
        data_directory: data.path().into(),
    }])
    .await
    .unwrap();
    (package, data, registry)
}

#[tokio::test]
async fn cancelled_call_is_reported_unavailable_without_another_invocation() {
    let (_package, _data, registry) = registry().await;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(50),
            registry.invoke("example", "admin.sleep", json!({}))
        )
        .await
        .is_err()
    );
    assert!(!registry.list().await[0].available);
    assert!(
        registry
            .invoke("example", "admin.echo", json!({}))
            .await
            .is_err()
    );
    registry.shutdown().await;
}

#[tokio::test]
async fn exited_process_is_reported_unavailable() {
    let (_package, _data, registry) = registry().await;
    registry
        .invoke("example", "admin.exit", json!({}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while registry.list().await[0].available {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    registry.shutdown().await;
}

#[tokio::test]
async fn shutdown_closes_admission_and_keeps_data() {
    let (_package, data, registry) = registry().await;
    assert!(matches!(
        registry.invoke("example", "admin.reject", json!({})).await,
        Err(gateway_admin::ports::plugins::PluginOperationError::Rejected)
    ));
    assert!(registry.list().await[0].available);
    assert_eq!(
        registry
            .invoke("example", "admin.echo", json!({"ok":true}))
            .await
            .unwrap(),
        json!({"ok":true})
    );
    std::fs::write(data.path().join("saved"), "retained").unwrap();
    registry.shutdown().await;
    assert!(!registry.list().await[0].available);
    assert!(
        registry
            .invoke("example", "admin.echo", json!({}))
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(data.path().join("saved")).unwrap(),
        "retained"
    );
}

#[tokio::test]
async fn rejects_cross_plugin_data_overlap_before_starting_programs() {
    // 无效可执行文件使任何意外启动返回 Io；正确预检应先返回 InvalidPackage。
    for nested_in_program in [false, true] {
        let first = super::package(b"must not execute");
        let second = super::package(b"must not execute");
        let manifest_path = second.path().join("plugin.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["id"] = "second".into();
        std::fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let data = tempfile::tempdir().unwrap();
        let second_data = if nested_in_program {
            first.path().join("data")
        } else {
            data.path().join("nested")
        };
        let result = PluginRegistry::start(vec![
            PluginConfig {
                directory: first.path().into(),
                data_directory: data.path().into(),
            },
            PluginConfig {
                directory: second.path().into(),
                data_directory: second_data,
            },
        ])
        .await;
        assert!(matches!(
            result,
            Err(gateway_host::plugins::PluginError::InvalidPackage)
        ));
    }
}
