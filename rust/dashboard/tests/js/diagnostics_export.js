const originalApi=window.pywebview.api;
const originalClipboard=Object.getOwnPropertyDescriptor(navigator,'clipboard');
const good=()=>({schema_version:1,app:{version:'2.3.0',platform:'windows',arch:'x86_64',build:'debug'},local_ai_setup:{scope:'last_recorded_in_this_dashboard_process',stage:'not_observed',issue:'none'}});
const check=(ok,msg)=>{if(!ok)throw Error(msg)};
const at=id=>document.getElementById(id);
const settings=document.querySelector('.toolnav button[data-tool="settings"]');
const overview=document.querySelector('.toolnav button[data-tool="overview"]');
const tick=()=>new Promise(resolve=>setTimeout(resolve,0));
const writes=[];
let reads=0,calls=0,denyCopy=false,copyGate=null,backend=async()=>good();
if(window.ipc?.postMessage){
  // Bypass the shim just for malformed-envelope coverage. The ordinary call below uses it.
  backend=()=>originalApi.diagnostics_export_preview();
  const savedResolve=window.__cu_resolve;
  const packets=[{}, {a:null}, {a:{}}, {a:0}, {a:[null]}, {a:['PRIVATE_ARG_CANARY']}];
  try{
    await new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>reject(Error('no reply to malformed preview IPC')),4000);
      const ids=new Set(packets.map((_,i)=>90000+i));
      window.__cu_resolve=(id,ok,v)=>{
        if(!ids.has(id))return savedResolve(id,ok,v);
        ids.delete(id);
        if(ok||v!=='diagnostics_unavailable'){clearTimeout(timer);reject(Error('malformed raw IPC was accepted'));return}
        if(!ids.size){clearTimeout(timer);resolve()}
      };
      packets.forEach((p,i)=>window.ipc.postMessage(JSON.stringify({id:90000+i,m:'diagnostics_export_preview',...p})));
    });
  } finally {window.__cu_resolve=savedResolve;}
}
window.pywebview.api=new Proxy(originalApi,{get(target,key){
 if(key==='diagnostics_export_preview')return (...args)=>{calls++;check(!args.length,'preview sent IPC args');return backend()};
 return target[key];
}});
Object.defineProperty(navigator,'clipboard',{configurable:true,value:{
 writeText:async text=>{if(denyCopy)throw Error('PRIVATE_ERROR_CANARY');if(copyGate)await copyGate;writes.push(text)},
 readText:async()=>{reads++;throw Error('cannot read clipboard')},
}});
try{
 settings.click();at('t-gen').click();await tick();
 check(calls===0&&writes.length===0&&reads===0,'opening Settings triggered collection or clipboard use');
 check(at('diag-report-copy').disabled&&at('diag-report-json').hidden,'no empty report copy');
 at('diag-report-preview').click();
 for(let i=0;i<150&&at('diag-report-json').hidden;i++)await new Promise(r=>setTimeout(r,10));
 const text=at('diag-report-json').value,parsed=JSON.parse(text);
 check(JSON.stringify(Object.keys(parsed))==='["schema_version","app","local_ai_setup"]','unexpected report keys');
 check(parsed.local_ai_setup.stage==='not_observed'&&parsed.local_ai_setup.issue==='none','unexpected setup history');
 check(!text.includes('CANARY')&&writes.length===0,'preview copied or exposed private configuration');
 at('diag-report-copy').click();await tick();
 check(writes.length===1&&writes[0]===text&&reads===0,'copy bytes did not match approved preview');
 denyCopy=true;at('diag-report-copy').click();await tick();denyCopy=false;
 check(!at('diag-report-status').textContent.includes('CANARY')&&writes.length===1,'clipboard failure leaked exception or wrote again');
 at('diag-report-clear').click();
 check(at('diag-report-copy').disabled&&at('diag-report-json').hidden&&!at('diag-report-json').value&&writes.length===1,'clear did not remove preview or unexpectedly altered clipboard');

 let getter=0;
 const inherited=Object.create({private:'PRIVATE_CANARY'});
 const accessor=good();
 Object.defineProperty(accessor.app,'version',{enumerable:true,get(){getter++;return '1.2.3'}});
 const extras=good();extras.app.prompt='PRIVATE_CANARY';
 const version=good();version.app.version='1.2.3\n';
 const unknown=good();unknown.local_ai_setup.issue='<img src=x onerror=alert("PRIVATE_CANARY")>';
 const oversized=good();oversized.app.version='0'.repeat(5000);
 for(const response of [null,inherited,accessor,extras,version,unknown,oversized]){
   backend=async()=>response;
   at('diag-report-preview').click();await tick();
   check(at('diag-report-copy').disabled&&at('diag-report-json').hidden&&!at('diag-report-json').value,'hostile response was exposed');
   check(!at('diag-report-status').textContent.includes('CANARY'),'hostile response leaked into status');
 }
 check(getter===0,'renderer invoked untrusted getter');
 // A same-prototype Proxy can expose benign own data to schema validation
 // but return arbitrary values/toJSON to a later serialization of the raw record.
 let unexpectedGets=0,unexpectedToJson=0;
 const hostileNested=target=>new Proxy(target,{
   get(_target,key){
     unexpectedGets++;
     if(key==='toJSON')return ()=>{unexpectedToJson++;return {private:'PRIVATE_SERIALIZATION_CANARY'}};
     return 'PRIVATE_PROPERTY_CANARY';
   },
 });
 backend=async()=>({
   local_ai_setup:hostileNested({issue:'none',stage:'not_observed',scope:'last_recorded_in_this_dashboard_process'}),
   app:hostileNested({build:'debug',arch:'x86_64',platform:'windows',version:'2.3.0'}),
   schema_version:1,
 });
 at('diag-report-preview').click();await tick();
 check(!at('diag-report-copy').disabled&&at('diag-report-json').value===JSON.stringify(good(),null,2),
   'validated nested data did not produce exactly the canonical inert report');
 check(unexpectedGets===0&&unexpectedToJson===0,'report accessed raw nested properties or toJSON');
 at('diag-report-clear').click();
 let late;
 backend=()=>new Promise(r=>{late=r});at('diag-report-preview').click();
 at('diag-report-clear').click();late(good());await tick();
 check(at('diag-report-copy').disabled&&at('diag-report-json').hidden,'cleared late reply was rendered');
 backend=()=>new Promise(r=>{late=r});at('diag-report-preview').click();
 overview.click();late(good());await tick();settings.click();
 check(at('diag-report-copy').disabled&&at('diag-report-json').hidden,'late reply populated a different route');
 backend=async()=>good();at('diag-report-preview').click();await tick();
 at('t-look').click();at('t-gen').click();
 check(at('diag-report-copy').disabled&&at('diag-report-json').hidden,'leaving Chung did not discard preview');
 check(reads===0&&writes.length===1,'non-copy actions touched clipboard');
 backend=async()=>good();at('diag-report-preview').click();await tick();
 const approved=at('diag-report-json').value;
 let allowCopy;
 copyGate=new Promise(resolve=>{allowCopy=resolve});
 at('diag-report-copy').click();
 at('diag-report-clear').click();allowCopy();await tick();
 check(writes.length===2&&writes[1]===approved&&at('diag-report-status').textContent===''&&at('diag-report-json').hidden,
   'clear resurrected preview or changed the bytes of an already requested copy');

 return {diagnostics_export:true};
}finally{
 window.pywebview.api=originalApi;
 if(originalClipboard)Object.defineProperty(navigator,'clipboard',originalClipboard);else delete navigator.clipboard;
}
