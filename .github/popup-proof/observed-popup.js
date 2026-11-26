// Observational uses of the production dashboard's existing debug test hook.
// No synthetic click or programmatic product intent. Native pointer drives actions.
const el = id => document.getElementById(id);
const route = () => document.body.classList.contains('ocr-popup') && el('tool-ocr').classList.contains('on') && !el('tool-translation').classList.contains('on');
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const wait = async (predicate, name) => {
 const until = performance.now() + 45000;
 while(performance.now() < until){
  const next = predicate(); if(next) return next;
  await pause(50);
 }
 throw Error(name+' failed: '+el('ocr-out').textContent+' / '+el('ocr-translation-out').textContent);
};
const point = button => {
 if(!route() || !button || button.disabled) throw Error('Not an enabled action in the OCR popup');
 button.scrollIntoView({block:'nearest',inline:'nearest'});
 const bounds = button.getBoundingClientRect();
 const x=bounds.left+bounds.width/2, y=bounds.top+bounds.height/2;
 if(!bounds.width||!bounds.height||!button.contains(document.elementFromPoint(x,y)))throw Error('Not a hit-testable action');
 return {x:Math.round(x*devicePixelRatio),y:Math.round(y*devicePixelRatio)};
};
const stage = async (name, data) => {
 await api.e2e_stage(name,data);
 const until=performance.now()+45000;
 while(performance.now()<until){if(await api.e2e_go(name))return;await pause(70);}
 throw Error('No native acknowledgement for '+name);
};
await wait(()=>route()&&!el('ocr-copy').disabled,'Automatic OCR');
const image=el('ocr-preview'); await image.decode();
const canvas=document.createElement('canvas'); canvas.width=image.naturalWidth; canvas.height=image.naturalHeight;
const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);
const pixels=ctx.getImageData(0,0,canvas.width,canvas.height).data;
let dark=0,white=0;
for(let i=0;i<pixels.length;i+=4){
 if(pixels[i]<40&&pixels[i+1]<40&&pixels[i+2]<40)dark++;
 if(pixels[i]>240&&pixels[i+1]>240&&pixels[i+2]>240)white++;
}
const ocr=el('ocr-out').textContent.trim();
await stage('copy',{point:point(el('ocr-copy')),ocr,width:canvas.width,height:canvas.height,dark,white});
await stage('translate',{point:point(document.querySelector('#tool-ocr [data-quick-translate]'))});
await wait(()=>route()&&!el('ocr-translation-copy').disabled,'Inline translation');
const vi=el('ocr-translation-out').textContent.trim();
await stage('translated-copy',{point:point(el('ocr-translation-copy')),vi,ocr:el('ocr-out').textContent.trim()});
await stage('warm-cancel-copy',{point:point(el('ocr-copy')),ocr:el('ocr-out').textContent.trim(),vi:el('ocr-translation-out').textContent.trim()});
await stage('warm-cancel-vi',{point:point(el('ocr-translation-copy')),ocr:el('ocr-out').textContent.trim(),vi:el('ocr-translation-out').textContent.trim()});
return {observed:true};
