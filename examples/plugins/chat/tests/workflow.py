"""真实插件进程 + 本机模拟 Responses 服务，不调用生产模型。"""
import json,struct,sys,subprocess,threading,http.server,time,pathlib,tempfile
seen=[]
class Handler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  assert self.path=='/v1/models';assert self.headers['Authorization']=='Bearer test-key'
  self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(json.dumps({'data':[{'id':'fixture-model'}]}).encode())
 def do_POST(self):
  body=json.loads(self.rfile.read(int(self.headers['Content-Length'])));seen.append(body)
  assert self.path=='/v1/responses';assert body['stream'] and body['store']==False
  if self.headers['Authorization']=='Bearer bad-key':
   self.send_response(401);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(b'{"error":{"message":"invalid bad-key"}}');return
  self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('x-request-id','fixture-request');self.end_headers()
  if body['model']=='slow':time.sleep(2)
  try:
   events=[{'type':'response.output_text.delta','delta':'你好'},{'type':'response.completed','response':{'id':'fixture-response','usage':{'input_tokens':3,'output_tokens':2}}}]
   wire=''.join('data: '+json.dumps(e,ensure_ascii=False)+'\r\n\r\n' for e in events).encode()
   for byte in wire:self.wfile.write(bytes([byte]));self.wfile.flush()
  except (BrokenPipeError,ConnectionResetError):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
with tempfile.TemporaryDirectory() as data:
 p=subprocess.Popen([str(pathlib.Path(sys.argv[1]).resolve())],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env={'RS_PLUGIN_DATA_DIR':data})
 seq=0
 def rpc(method,params):
  global seq
  seq+=1;b=json.dumps({'jsonrpc':'2.0','id':seq,'method':method,'params':params}).encode();p.stdin.write(struct.pack('>I',len(b))+b);p.stdin.flush();prefix=p.stdout.read(4);assert len(prefix)==4,'process ended';reply=json.loads(p.stdout.read(struct.unpack('>I',prefix)[0]));assert 'error' not in reply,reply;return reply['result']
 def start(kind,model='fixture-model',key='test-key',messages=None):return rpc('admin.start',{'kind':kind,'port':server.server_port,'apiKey':key,'model':model,'messages':messages or [{'role':'user','content':'hello'}]})['jobId']
 def done(id):
  for _ in range(100):
   result=rpc('admin.poll',{'jobId':id})
   if result['state']!='running':return result
   time.sleep(.02)
  raise Exception('job did not finish')
 try:
  assert rpc('initialize',{'apiVersion':1,'pluginId':'chat-test'})['apiVersion']==1
  assert '对话测试' in rpc('admin.ui',{})['html']
  assert done(start('models'))['models']==['fixture-model']
  result=done(start('chat'));assert result['text']=='你好' and result['state']=='completed' and result['usage']['output_tokens']==2,result
  assert result['requestId']=='fixture-request'
  result=done(start('chat',messages=[{'role':'user','content':'first'},{'role':'assistant','content':'answer'},{'role':'user','content':'next'}]));assert result['state']=='completed';assert seen[-1]['input'][1]['content'][0]['type']=='output_text'
  result=done(start('chat',key='bad-key'));assert result['state']=='failed' and result['httpStatus']==401 and 'bad-key' not in json.dumps(result)
  id=start('chat',model='slow');assert rpc('admin.cancel',{'jobId':id})['state']=='cancelled'
  assert list(pathlib.Path(data).iterdir())==[]
  print('PASS handshake, UI, models, UTF-8 split SSE, multi-turn payload, usage, auth error redaction, cancellation, no disk persistence')
 finally:p.kill();p.wait();server.shutdown()
