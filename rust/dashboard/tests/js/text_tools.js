const node = id => document.getElementById(id);
const check = (ok, message) => { if (!ok) throw Error(message); };
const sleep = ms => new Promise(resolve=>setTimeout(resolve,ms));
const requests=[],clipboard=[];
const originalApi=window.pywebview.api, originalClipboard=Object.getOwnPropertyDescriptor(navigator,'clipboard');
window.pywebview.api=new Proxy(originalApi,{
 get(target,key){
  if(key==='local_chat')return arg=>new Promise(resolve=>requests.push({arg,resolve}));
  return target[key];
 }
});
Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>clipboard.push(text)}});
const input=(id,value,event='input')=>{
 const e=node(id);
 if(e.value!==undefined)e.value=value;else e.textContent=value;
 e.dispatchEvent(new Event(event,{bubbles:true}));
};
const phase=kind=>node(kind+'-status').dataset.state;
try{
 toolTab('grammar');
 check(node('grammar-run').disabled&&node('grammar-copy').disabled,'empty grammar controls');
 input('grammar-in','She walk.');
 node('grammar-run').click();await sleep(0);
 check(requests[0].arg.task==='grammar'&&requests[0].arg.text==='She walk.'&&phase('grammar')==='processing','grammar processing');
 requests[0].resolve({text:'She walks.'});await sleep(0);
 check(phase('grammar')==='ready'&&node('grammar-out').textContent==='She walks.'&&!node('grammar-copy').disabled,'grammar ready for edit/copy');
 input('grammar-out','  My edit\nsecond line  ');
 node('grammar-run').click();await sleep(0);
 check(!node('grammar-copy').disabled&&phase('grammar')==='processing','edited output must stay copyable while running');
 node('grammar-copy').click();await sleep(0);
 check(clipboard[0]==='  My edit\nsecond line  ','clipboard did not preserve exact edit/whitespace');
 requests[1].resolve({code:'timeout'});await sleep(0);
 check(phase('grammar')==='error'&&node('grammar-run').textContent==='Thử lại'&&node('grammar-out').textContent==='  My edit\nsecond line  ','failure destroyed editable result');
 node('grammar-run').click();await sleep(0);
 input('grammar-out','New user edit');
 requests[2].resolve({text:'Late correction'});await sleep(0);
 check(phase('grammar')==='edited'&&node('grammar-out').textContent==='New user edit','late inference overwrote an in-flight edit');
 node('grammar-run').click();await sleep(0);
 input('grammar-in','A newer draft.');
 requests[3].resolve({code:'timeout'});await sleep(0);
 check(phase('grammar')==='stale'&&node('grammar-in').value==='A newer draft.'&&node('grammar-out').textContent==='New user edit','stale failure overwrote status, input or output');

 toolTab('translation');
 check(node('translation-lang').value==='vi','Vietnamese must default');
 openQuickTool('translation','  ');
 check(phase('translation')==='paste'&&document.activeElement===node('translation-in')&&requests.length===4,'blank selection must focus paste with no inference');
 input('translation-in','Close the door.');
 node('translation-run').click();await sleep(0);
 check(requests[4].arg.target==='vi','default target not sent');
 input('translation-lang','en','change');
 requests[4].resolve({text:'Đóng cửa.'});await sleep(0);
 check(!node('translation-out').textContent&&phase('translation')==='draft','stale result from former target');
 openQuickTool('translation','Xin chào.');
 await sleep(0);
 check(requests[5].arg.target==='en','English option not sent');
 openQuickTool('translation','Chào buổi sáng.');
 await sleep(0);
 check(requests.length===6&&phase('translation')==='queued','latest selected text must wait behind native single-flight');
 requests[5].resolve({text:'Stale Hello'});await sleep(0);
 check(requests.length===7&&requests[6].arg.text==='Chào buổi sáng.'&&!node('translation-out').textContent,'queued shortcut did not supersede old request');
 requests[6].resolve({text:'Good morning.'});await sleep(0);
 input('translation-out','  Good morning!\n');
 node('translation-copy').click();await sleep(0);
 check(clipboard[1]==='  Good morning!\n','edited English translation not copied exactly');
 openQuickTool('translation','');
 check(node('translation-in').value==='Chào buổi sáng.'&&node('translation-out').textContent==='  Good morning!\n'&&requests.length===7,'no-selection shortcut discarded draft or made a request');
 return {text_tools:true};
}finally{
 window.pywebview.api=originalApi;
 if(originalClipboard)Object.defineProperty(navigator,'clipboard',originalClipboard);else delete navigator.clipboard;
}
