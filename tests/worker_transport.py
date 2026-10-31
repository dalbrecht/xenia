"""The actual worker client rejects untrusted TLS and follows no redirects."""
import http.server, os, pathlib, ssl, subprocess, tempfile, threading
binary=str(pathlib.Path(os.environ.get('XENIA_TEST_BINARY','target/debug/xenia')).resolve())
with tempfile.TemporaryDirectory(prefix='xenia-worker-tls-') as tmp:
    root=pathlib.Path(tmp); requests=[]
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self,*args):pass
        def do_POST(self):
            requests.append(self.path)
            self.rfile.read(int(self.headers.get('Content-Length','0')))
            self.send_response(302);self.send_header('Location',f'http://127.0.0.1:{self.server.server_port}/unexpected');self.send_header('Content-Length','0');self.end_headers()
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
    thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
    env=os.environ|{'XENIA_CONTROLLER':f'http://127.0.0.1:{server.server_port}','XENIA_NODE_ID':'transport-test'}
    try:
        result=subprocess.run([binary,'pair'],env=env,capture_output=True,text=True,timeout=20)
        assert result.returncode!=0 and '302' in result.stderr and requests==['/api/node-pairing/request'],(result.stderr,requests)
    finally: server.shutdown();server.server_close();thread.join()
    subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-keyout',str(root/'key.pem'),'-out',str(root/'cert.pem'),'-days','1','-subj','/CN=localhost'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
    tls=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);tls.load_cert_chain(root/'cert.pem',root/'key.pem');server.socket=tls.wrap_socket(server.socket,server_side=True)
    thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();requests.clear()
    try:
        env['XENIA_CONTROLLER']=f'https://localhost:{server.server_port}'
        result=subprocess.run([binary,'pair'],env=env,capture_output=True,text=True,timeout=20)
        assert result.returncode!=0 and not requests and ('certificate' in result.stderr.lower() or 'tls' in result.stderr.lower()),result.stderr
        print('Worker transport: redirects refused and untrusted TLS rejected before sending HTTP data: PASS')
    finally:server.shutdown();server.server_close();thread.join()
