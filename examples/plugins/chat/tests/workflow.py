"""真实插件进程握手/隔离页面/拒绝旧密钥 RPC；请求桥另做浏览器验收。"""
import json,struct,subprocess,sys,tempfile,pathlib
with tempfile.TemporaryDirectory() as data:
 p=subprocess.Popen([str(pathlib.Path(sys.argv[1]).resolve())],stdin=subprocess.PIPE,stdout=subprocess.PIPE,env={'RS_PLUGIN_DATA_DIR':data})
 def rpc(method,params):
  raw=json.dumps({'jsonrpc':'2.0','id':1,'method':method,'params':params}).encode();p.stdin.write(struct.pack('>I',len(raw))+raw);p.stdin.flush();length=struct.unpack('>I',p.stdout.read(4))[0];return json.loads(p.stdout.read(length))
 try:
  assert rpc('initialize',{'apiVersion':1,'pluginId':'chat-test'})['result']['pluginId']=='chat-test'
  page=rpc('admin.ui',{})['result']['html'];assert 'chat.context' in page and 'type="password"' not in page
  assert 'error' in rpc('admin.start',{'apiKey':'fixture-not-real'})
  assert list(pathlib.Path(data).iterdir())==[]
  print('PASS handshake, UI host bridge, no password field, legacy secret RPC rejected, no disk persistence')
 finally:p.kill();p.wait()
