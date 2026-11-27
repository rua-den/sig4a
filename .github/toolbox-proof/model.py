"""Loopback model transport double for a synthetic native desktop, not a model-accuracy test."""
import base64,json,struct,sys,threading
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from hashlib import sha256
from pathlib import Path

MODEL='sig4a-toolbox-fixture'
LOCK=threading.Lock()
OCR_RESULT=None

class Handler(BaseHTTPRequestHandler):
    def output(self, val, status=200):
        body=json.dumps(val).encode()
        self.send_response(status); self.send_header('Content-Type','application/json')
        self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
    def do_GET(self):
        if self.path not in ('/health','/v1/models'): self.output({},404); return
        self.output({'data':[{'id':MODEL}]})
    def do_POST(self):
        global OCR_RESULT
        try:
            assert self.path=='/v1/chat/completions'
            n=int(self.headers['Content-Length']); assert 1<=n<=8*1024*1024
            req=json.loads(self.rfile.read(n))
            assert req['model']==MODEL
            msg=req['messages'][-1]['content']
            image_sha=None
            if isinstance(msg,list):
                task='ocr'; assert 'Transcribe' in msg[0]['text']
                uri=msg[1]['image_url']['url']; hdr,data=uri.split(',',1)
                assert hdr=='data:image/png;base64'
                png=base64.b64decode(data,validate=True)
                assert png[:8]==b'\x89PNG\r\n\x1a\n' and png[12:16]==b'IHDR'
                w,h=struct.unpack('>II',png[16:24]); assert abs(w-360)<=1 and abs(h-85)<=1, (w,h)
                image_sha=sha256(png).hexdigest()[:16]; source='native PNG'; text='OCR '+image_sha
                OCR_RESULT=text
            elif 'Translate the following source text' in msg:
                task='translate'; source=msg.split('Source text:\n',1)[1].strip()
                assert source in {'FixtureInputAlpha','ManualDraftAlpha','DashboardDraftAlpha',OCR_RESULT}, source
                text='VI '+source
            else:
                task='grammar'; source=msg.split('\n\n',1)[1].strip()
                assert 'Correct the English grammar' in msg and source in {'FixtureInputAlpha','DashboardDraftAlpha'}, source
                text='Fixed '+source
            with LOCK,path.open('a',encoding='ascii') as f:
                print(json.dumps({'task':task,'source':source,'result':text,'sha':image_sha}),file=f,flush=True)
            self.output({'choices':[{'message':{'content':text}}]})
        except Exception as e:
            print('Invalid synthetic fixture request:',repr(e),file=sys.stderr,flush=True)
            self.output({'error':'invalid fixture input'},400)

if __name__=='__main__':
    path=Path(sys.argv[2]); server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
    temp=Path(sys.argv[1]+'.tmp'); temp.write_text(str(server.server_port),encoding='ascii')
    temp.replace(Path(sys.argv[1]));server.serve_forever()
