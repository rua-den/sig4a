import http from 'node:http';
import fs from 'node:fs';
const port=Number(process.argv[2]||18229), log=process.argv[3];
const server=http.createServer(async(req,res)=>{
 res.setHeader('content-type','application/json');
 if(req.method==='GET'&&req.url==='/api/tags')return res.end(JSON.stringify({models:[{name:'sig4a-fixture'}]}));
 if(req.method!=='POST'||req.url!=='/api/chat'){res.statusCode=404;return res.end('{}')}
 let raw='';
 for await(const b of req){raw+=b;if(raw.length>1048576){res.statusCode=413;return res.end('{}')}}
 try{
  const data=JSON.parse(raw),message=data.messages?.[0],content=message?.content??'';
  if(data.model!=='sig4a-fixture')throw new Error('unexpected model');
  const text=message.images?.length?'Fixture OCR handoff.'
   :content.startsWith('Correct')?'Corrected fixture grammar.'
   :content.includes(' to English.')?'Fixture translation to English.'
   :content.includes(' to Vietnamese.')?'Fixture translation to Vietnamese.':null;
  if(log&&text)fs.appendFileSync(log,text+'\n');
  if(!text)throw new Error('unexpected fixture task');
  res.end(JSON.stringify({message:{content:text},done:true}));
 }catch(e){res.statusCode=400;res.end(JSON.stringify({error:String(e)}));}
});
server.listen(port,'127.0.0.1',()=>console.log('fixture provider listening'));
