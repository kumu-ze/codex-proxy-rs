use gateway_host::plugins::{PluginError, PluginPackage, PluginProcess};
use serde_json::json;
use std::time::Duration;

// 测试夹具使用独立 Python 进程，验证真实管道的分帧与生命周期。
const SCRIPT: &[u8] = br#"#!/usr/bin/python3
import sys, struct, json, time, os
while True:
    prefix = sys.stdin.buffer.read(4)
    if len(prefix) != 4: break
    request = json.loads(sys.stdin.buffer.read(struct.unpack('>I',prefix)[0]))
    method = request['method']
    if method == 'reject':
        body = json.dumps({'jsonrpc':'2.0','id':request['id'],'error':{'code':-32601,'message':'private diagnostic'}}).encode()
        sys.stdout.buffer.write(struct.pack('>I',len(body))+body); sys.stdout.buffer.flush(); continue
    if method == 'sleep': time.sleep(2)
    if method == 'oversize':
        sys.stdout.buffer.write(struct.pack('>I', 1048577)); sys.stdout.buffer.flush(); continue
    result = request['params'] if method != 'environment' else sorted(os.environ.keys())
    body = json.dumps({'jsonrpc':'2.0','id':request['id'] + (1 if method == 'wrong-id' else 0),'result':result}).encode()
    sys.stdout.buffer.write(struct.pack('>I',len(body))+body); sys.stdout.buffer.flush()
"#;

async fn start(dir: &tempfile::TempDir) -> PluginProcess {
    let package = PluginPackage::open(dir.path()).unwrap();
    PluginProcess::start(&package, dir.path(), Duration::from_secs(3))
        .await
        .unwrap()
}

#[tokio::test]
async fn roundtrip_keeps_environment_private() {
    let dir = super::package(SCRIPT);
    let mut process = start(&dir).await;
    assert_eq!(
        process
            .call("reject", json!({}), Duration::from_secs(1))
            .await
            .unwrap_err(),
        PluginError::Rejected
    );
    assert!(process.is_available());
    assert_eq!(
        process
            .call("echo", json!({"value":42}), Duration::from_secs(1))
            .await
            .unwrap(),
        json!({"value":42})
    );
    let environment = process
        .call("environment", json!({}), Duration::from_secs(1))
        .await
        .unwrap();
    assert!(
        environment
            .as_array()
            .unwrap()
            .iter()
            .all(|key| matches!(key.as_str(), Some("RS_PLUGIN_DATA_DIR" | "LC_CTYPE")))
    );
    process.stop().await.unwrap();
}

#[tokio::test]
async fn rejects_bad_frames_and_retires_timed_out_processes() {
    for (method, expected) in [
        ("oversize", PluginError::Protocol),
        ("wrong-id", PluginError::Protocol),
        ("sleep", PluginError::Timeout),
    ] {
        let dir = super::package(SCRIPT);
        let mut process = start(&dir).await;
        assert_eq!(
            process
                .call(method, json!({}), Duration::from_millis(50))
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(
            process
                .call("echo", json!({}), Duration::from_secs(1))
                .await
                .unwrap_err(),
            PluginError::Stopped
        );
    }
}

#[tokio::test]
async fn cancelled_request_cannot_leak_response_into_next_call() {
    let dir = super::package(SCRIPT);
    let mut process = start(&dir).await;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(50),
            process.call("sleep", json!({}), Duration::from_secs(3))
        )
        .await
        .is_err()
    );
    assert_eq!(
        process
            .call("echo", json!({}), Duration::from_secs(1))
            .await
            .unwrap_err(),
        PluginError::Stopped
    );
}
