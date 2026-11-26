import fs from 'node:fs';
let tab;
for(let i=0;i<25&&!tab;i++){
 try{tab=(await(await fetch('http://127.0.0.1:18227/json/list',{signal:AbortSignal.timeout(1200)})).json()).find(x=>x.type==='page')}catch{}
 if(!tab)await new Promise(r=>setTimeout(r,250));
}
if(!tab)throw Error('No loopback WebView2 target');
const url=new URL(tab.webSocketDebuggerUrl);
if(url.protocol!=='ws:'||url.hostname!=='127.0.0.1'||url.port!=='18227')throw Error('Unexpected non-loopback debugger');
const ws=new WebSocket(url);
try{
 await new Promise((r,j)=>{ws.onopen=r;ws.onerror=j});
 const value=await new Promise((r,j)=>{
  const t=setTimeout(()=>j(Error('CDP timed out')),18000);
  ws.onmessage=e=>{const m=JSON.parse(e.data);if(m.id!==1)return;clearTimeout(t);if(m.error||m.result.exceptionDetails)j(Error(JSON.stringify(m.error||m.result.exceptionDetails)));else r(m.result.result.value??null)};
  ws.send(JSON.stringify({id:1,method:'Runtime.evaluate',params:{expression:fs.readFileSync(process.argv[2],'utf8'),awaitPromise:true,returnByValue:true}}))
 });
 console.log(JSON.stringify(value));
}finally{ws.close()}
