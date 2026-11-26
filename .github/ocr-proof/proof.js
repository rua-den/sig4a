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
return {width:sample.width,height:sample.height,dark,white,text,translated,translationInput:document.getElementById('translation-in').value};
