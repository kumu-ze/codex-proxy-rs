import hashlib,json,pathlib,shutil,sys,tarfile
binary=pathlib.Path(sys.argv[1]);output=pathlib.Path(sys.argv[2]);output.mkdir(parents=True,exist_ok=True)
shutil.copy2(binary,output/'worker');(output/'worker').chmod(0o700)
manifest={'id':'chat-test','version':'0.1.0','apiVersion':1,'menuLabel':'对话测试','executable':'worker','files':{'worker':hashlib.sha256((output/'worker').read_bytes()).hexdigest()}}
(output/'plugin.json').write_text(json.dumps(manifest,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
with tarfile.open(output.parent/'rs-chat-test-0.1.0-linux-x64.tar.gz','w:gz') as tar:
 for name in ['worker','plugin.json']:tar.add(output/name,arcname=name)
