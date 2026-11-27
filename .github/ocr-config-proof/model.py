"""Local deterministic transport double: accepts only a real SIG4A PNG crop of expected size.
It proves native focus/interaction and retention of the resulting text, NOT OCR accuracy.
Binds only to loopback and receives only the synthetic Windows fixture.
"""
import base64,json,struct,sys
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from hashlib import sha256
from pathlib import Path

class Handler(BaseHTTPRequestHandler):
    def output(self, val, status=200):
        body=json.dumps(val).encode()
        self.send_response(status); self.send_header('Content-Type','application/json')
        self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
    def do_GET(self):
        if self.path not in ('/health','/v1/models'): self.output({},404); return
        self.output({'data':[{'id':'sig4a-focus-fixture'}]})
    def do_POST(self):
        try:
            assert self.path=='/v1/chat/completions'
            n=int(self.headers['Content-Length']); assert 1<=n<=8*1024*1024
            req=json.loads(self.rfile.read(n))
            assert req['model']=='sig4a-focus-fixture'
            msg=req['messages'][-1]['content']
            assert 'Transcribe' in msg[0]['text']
            uri=msg[1]['image_url']['url']; hdr,data=uri.split(',',1)
            assert hdr=='data:image/png;base64'
            png=base64.b64decode(data,validate=True)
            assert png[:8]==b'\x89PNG\r\n\x1a\n' and png[12:16]==b'IHDR'
            w,h=struct.unpack('>II',png[16:24]); assert abs(w-360)<=1 and abs(h-85)<=1
            text='Fixture OCR '+sha256(png).hexdigest()[:16]
            # One request at a time from this dedicated test profile. Write atomically before replying.
            temp=Path(str(observed)+'.new'); temp.write_text(text,encoding='ascii'); temp.replace(observed)
            self.output({'choices':[{'message':{'content':text}}]})
        except Exception as e:
            print('Invalid synthetic fixture request:',repr(e),file=sys.stderr,flush=True)
            self.output({'error':'invalid fixture image'},400)

if __name__=='__main__':
    observed=Path(sys.argv[2]); server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
    Path(sys.argv[1]).write_text(str(server.server_port),encoding='ascii')
    server.serve_forever()
