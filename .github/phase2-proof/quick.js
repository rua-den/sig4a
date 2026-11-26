const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const active=()=>document.querySelector('.toolpanel.on')?.id;
for(let i=0;i<55&&!document.body.classList.contains('route-ai');i++)await sleep(100);
const tool=active();
if(!window.__QUICK_MODE__||!document.body.classList.contains('quick')||getComputedStyle(document.querySelector('.accent-picker')).display!=='none')throw Error('Not a real compact tool');
if(tool==='tool-grammar'){
 const text=document.querySelector('#grammar-in').value;
 if(text!=='Synthetic phase two selection')throw Error('UIA selected text mismatch: '+text);
 return {tool,text};
}
if(tool==='tool-translation'){
 const el=document.querySelector('#translation-in');
 if(el.value!==''||document.activeElement!==el)throw Error('No-selection fallback did not focus the empty translation input');
 await api.e2e_stage('paste',{ready:true});
 for(let i=0;i<100&&el.value!=='Synthetic phase two paste';i++)await sleep(100);
 if(el.value!=='Synthetic phase two paste')throw Error('Native clipboard paste did not reach the translation input');
 return {tool,text:el.value};
}
if(tool==='tool-ocr'){
 const status=document.getElementById('ocr-status');
 for(let i=0;i<240&&!status.textContent.includes('huỷ');i++)await sleep(100);
 if(!status.textContent.includes('huỷ')||document.getElementById('ocr-preview').getAttribute('src'))throw Error('Capture did not cancel cleanly: '+status.textContent);
 return {tool,status:status.textContent,noImage:true};
}
throw Error('Wrong quick route '+tool);
