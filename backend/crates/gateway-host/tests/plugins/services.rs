use gateway_admin::ports::plugins::PluginOperations;
use gateway_core::engine::extensions::{ExtensionServices, ExtensionUnavailable};
use gateway_host::plugins::{PluginConfig, PluginRegistry};
use serde_json::{Value, json};
use std::sync::Arc;

const SCRIPT:&[u8]=br#"#!/usr/bin/python3
import sys,struct,json,os,urllib.request,urllib.error
while True:
 p=sys.stdin.buffer.read(4)
 if not p:break
 r=json.loads(sys.stdin.buffer.read(struct.unpack('>I',p)[0]))
 result=r['params']
 if r['method']=='admin.call':
  token=os.environ['RS_PLUGIN_SERVICES_TOKEN'] if r['params']['authorized'] else 'invalid'
  request=urllib.request.Request(os.environ['RS_PLUGIN_SERVICES_URL'],data=json.dumps({'method':'test','input':{'value':42}}).encode(),headers={'Authorization':token,'Content-Type':'application/json'})
  try:
   with urllib.request.urlopen(request) as response: result=json.load(response)
  except urllib.error.HTTPError as e:result={'status':e.code}
 body=json.dumps({'jsonrpc':'2.0','id':r['id'],'result':result}).encode();sys.stdout.buffer.write(struct.pack('>I',len(body))+body);sys.stdout.buffer.flush()
"#;
struct Services;
#[async_trait::async_trait]
impl ExtensionServices for Services {
    async fn invoke(&self, method: &str, input: Value) -> Result<Value, ExtensionUnavailable> {
        assert_eq!(method, "test");
        Ok(input)
    }
}
#[tokio::test]
async fn provider_callback_requires_private_capability_token() {
    let package = super::package(SCRIPT);
    let data = tempfile::tempdir().unwrap();
    let path = package.path().join("plugin.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["capabilities"] = json!(["provider.openai"]);
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let registry = PluginRegistry::start(vec![PluginConfig {
        directory: package.path().into(),
        data_directory: data.path().into(),
    }])
    .await
    .unwrap();
    assert_eq!(
        registry
            .invoke("example", "admin.call", json!({"authorized":false}))
            .await
            .unwrap()["status"],
        401
    );
    registry.attach_services(Arc::new(Services)).unwrap();
    assert_eq!(
        registry
            .invoke("example", "admin.call", json!({"authorized":true}))
            .await
            .unwrap(),
        json!({"value":42})
    );
    registry.shutdown().await;
}
