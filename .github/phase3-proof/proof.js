const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, label) {
  for (let i = 0; i < 400; i++) {
    if (check()) return;
    await sleep(100);
  }
  throw new Error(label);
}
async function stage(name, data) {
  await api.e2e_stage(name, data);
  await untilAsync(name);
}
async function untilAsync(name) {
  for (let i = 0; i < 200; i++) {
    if (await api.e2e_go(name)) return;
    await sleep(100);
  }
  throw new Error('Driver did not complete '+name);
}
const setupButton = document.getElementById('ai-test');
if(!setupButton)throw Error('Phase 3 Test button missing');
{
 toolTab('grammar');
 const grammar = document.getElementById('grammar-in');
 grammar.value = 'Synthetic recovery input';
 grammar.dispatchEvent(new Event('input',{bubbles:true}));
 toolTab('localai');
 const endpoint = document.getElementById('ai-endpoint');
 const status = document.getElementById('ai-status');
 const check = document.getElementById('ai-check');
 const model = document.getElementById('ai-model');
 const base = endpoint.value;
 if(base!=='http://127.0.0.1:18080'||model.value!=='qwen3-vl-2b')throw Error('Unexpected scratch configuration: '+base+' '+model.value);
 function edit(el,v){el.value=v;el.dispatchEvent(new Event('input',{bubbles:true}));}
 edit(endpoint, 'http://127.0.0.1:18290');
 check.click();
 await until(()=>!check.disabled&&status.dataset.state==='unreachable','Unavailable loopback endpoint was not diagnosed: '+status.dataset.state+' '+status.textContent);
 const unavailable=status.textContent.trim();
 if(!unavailable)throw Error('Unavailable diagnosis missing a readable message');
 edit(endpoint,base);
 check.click();
 await until(()=>!check.disabled&&(status.dataset.state==='connected'||status.dataset.state==='ready'),'Live model list connection not observed: '+status.dataset.state+' '+status.textContent);
 setupButton.click();
 await until(()=>!setupButton.disabled&&status.dataset.state==='ready','Real embedded vision test did not mark Ready: '+status.dataset.state+' '+status.textContent);
 const ready=status.textContent.trim();
 document.getElementById('ai-save').click();
 await until(()=>!document.getElementById('ai-save').disabled,'Save did not settle');
 const cfg=(await api.state()).cfg;
 if(cfg.local_ai_endpoint!==base||cfg.local_ai_model!=='qwen3-vl-2b')throw Error('Successful Local AI settings did not save');
 toolTab('grammar');
 if(grammar.value!=='Synthetic recovery input')throw Error('Recovery navigation discarded unsent input');
 await api.e2e_stage('recovery_verified',{unavailable,ready,sessionInputPreserved:true,loopbackSaved:cfg.local_ai_endpoint});
}
toolTab('ocr');
await stage('capture', {});
const captureButton = document.querySelector('#tool-ocr button[onclick="captureOcr()"]');
if (!captureButton) throw new Error('Capture button was missing');
captureButton.click();
await api.e2e_stage('capture_started', { status: document.getElementById('ocr-status').textContent });
await until(() => !document.getElementById('ocr-run').disabled || /huỷ|Không chụp/.test(document.getElementById('ocr-status').textContent), 'Capture did not finish');
const img = document.getElementById('ocr-preview');
if (!img.getAttribute('src')?.startsWith('data:image/png;base64,')) throw new Error('Native capture: '+document.getElementById('ocr-status').textContent);
await img.decode();
const sample = document.createElement('canvas');
sample.width = img.naturalWidth;
sample.height = img.naturalHeight;
const ctx = sample.getContext('2d');
ctx.drawImage(img, 0, 0);
const pixels=ctx.getImageData(0,0,sample.width,sample.height).data;
let dark=0, white=0;
for(let i=0;i<pixels.length;i+=4){
  if(pixels[i]<40&&pixels[i+1]<40&&pixels[i+2]<40)dark++;
  if(pixels[i]>240&&pixels[i+1]>240&&pixels[i+2]>240)white++;
}
if(sample.width!==460||sample.height!==110||dark<100||white<20000)throw new Error(JSON.stringify({width:sample.width,height:sample.height,dark,white}));
document.getElementById('ocr-run').click();
await until(() => !document.getElementById('ocr-run').disabled, 'OCR did not finish');
const text = document.getElementById('ocr-out').textContent.trim();
if (!/^close the door[.!]?$/i.test(text)) throw new Error('Unexpected OCR: '+text);
const copy = document.getElementById('ocr-copy');
if(copy.disabled)throw new Error('Copy was disabled after successful OCR');
const r = copy.getBoundingClientRect();
await stage('copy', {text,x:Math.round((r.x+r.width/2)*devicePixelRatio),y:Math.round((r.y+r.height/2)*devicePixelRatio)});
document.querySelector('#tool-ocr [data-quick-translate]').click();
await until(() => !document.getElementById('translation-run').disabled, 'Translation did not finish');
const translated = document.getElementById('translation-out').textContent.trim();
const captureTranslationInput = document.getElementById('translation-in').value;
if(captureTranslationInput!==text)throw Error('Capture OCR was not handed to Translate');
await runTool('translation');
const translatedRepeat = document.getElementById('translation-out').textContent.trim();
toolTab('ocr');
const priorSrc = img.getAttribute('src');
document.getElementById('ocr-in').focus();
await stage('paste', {});
await until(() => img.getAttribute('src') && img.getAttribute('src')!==priorSrc, 'Actual clipboard bitmap did not replace the captured image');
if(!copy.disabled||!document.querySelector('#tool-ocr [data-quick-translate]').disabled)throw Error('Actions remained enabled on replacement image');
await img.decode();
if(img.naturalWidth!==460||img.naturalHeight!==110)throw Error('Unexpected clipboard bitmap size');
document.getElementById('ocr-run').click();
await until(() => !document.getElementById('ocr-run').disabled,'Pasted OCR did not finish');
const pastedText=document.getElementById('ocr-out').textContent.trim();
if(!/^open the window[.!]?$/i.test(pastedText)||copy.disabled)throw Error('Pasted image OCR: '+pastedText);
const savedImage=img.getAttribute('src');
await stage('cancel', {});
captureButton.click();
await until(()=>/huỷ/.test(document.getElementById('ocr-status').textContent),'Native picker did not cancel');
if(img.getAttribute('src')!==savedImage||document.getElementById('ocr-out').textContent.trim()!==pastedText||copy.disabled)throw Error('Cancel discarded current image or output');
const pr=copy.getBoundingClientRect();
await stage('pasted_copy',{text:pastedText,x:Math.round((pr.x+pr.width/2)*devicePixelRatio),y:Math.round((pr.y+pr.height/2)*devicePixelRatio)});
document.querySelector('#tool-ocr [data-quick-translate]').click();
await until(()=>!document.getElementById('translation-run').disabled,'Pasted OCR translation did not finish');
const translatedPaste=document.getElementById('translation-out').textContent.trim();
const pasteTranslationInput=document.getElementById('translation-in').value;
if(pasteTranslationInput!==pastedText)throw Error('Pasted OCR was not handed to Translate');
toolTab('ocr');
const late=runTool('ocr');
removeOcr();
await late;
if(img.getAttribute('src')||!copy.disabled||!document.querySelector('#tool-ocr [data-quick-translate]').disabled||document.getElementById('ocr-out').textContent.includes(pastedText))throw Error('Removing in-flight image allowed late result');
return {width:sample.width,height:sample.height,dark,white,text,translated,translatedRepeat,translationInput:captureTranslationInput,pastedText,translatedPaste,pasteTranslationInput,cancelPreserved:true,lateDiscarded:true};
