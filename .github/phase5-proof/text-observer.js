// Observational stage hooks only. Native Windows pointer and keys perform actions.
const el=id=>document.getElementById(id);
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const assert=(ok,msg)=>{if(!ok)throw new Error(msg)};
const visible=e=>!!e&&e.getClientRects().length>0;
async function until(check,label,ms=18000){
 const end=performance.now()+ms;
 while(performance.now()<end){const result=await check();if(result)return result;await sleep(50);}
 throw Error('Timeout '+label);
}
function point(e){
 assert(visible(e)&&!e.disabled,'Unavailable target: '+e?.id);
 e.scrollIntoView({block:'nearest',inline:'nearest'});
 const b=e.getBoundingClientRect(),x=b.left+b.width/2,y=b.top+b.height/2;
 assert(b.width&&b.height&&e.contains(document.elementFromPoint(x,y)),'Unclickable: '+e.id);
 return {x:Math.round(x*devicePixelRatio),y:Math.round(y*devicePixelRatio)}
}
async function stage(name,value={}){await api.e2e_stage(name,value);await until(()=>api.e2e_go(name),name,30000)}
const input=k=>el(k+'-in'),out=k=>el(k+'-out'),run=k=>el(k+'-run');
const result=k=>out(k).textContent.trim();
const copy=k=>el(k+'-copy')||document.querySelector('#tool-'+k+' [data-copy],#tool-'+k+' button[onclick*="copyResult"]');
const shown=k=>el('tool-'+k)?.classList.contains('on');
const expected=k=>k==='grammar'?'Corrected fixture grammar.':'Fixture translation to English.';
const edited=k=>k==='grammar'?'Edited Grammar Result.':'Edited Translation Result.';
async function textFlow(k,{prefix='',manual=false,quick=false}={}){
 const key=n=>prefix+n;
 if(quick&&!manual&&k==='translation'){
  await until(()=>result(k)==='Fixture translation to Vietnamese.','Vietnamese-default quick translation');
 }
 if(manual){
  if(quick){
   assert(document.activeElement===input(k),'No-selection did not focus the paste target');
   assert(!input(k).value,'No-selection unexpectedly supplied text');
  }
  await stage(key('paste'),point(input(k)));
  await until(()=>input(k).value.includes('fixture'),key('pasted'));
 }
 if(k==='translation'){
  assert(el('translation-lang').value==='vi','Broke default Vietnamese target');
  await stage(key('english'),point(el('translation-lang')));
  await until(()=>el('translation-lang').value==='en','native target selection');
 }
 if(!quick||manual||k==='translation')await stage(key('run'),point(run(k)));
 await until(()=>result(k)===expected(k),key('local result'));
 await stage(key('edit'),point(out(k)));
 await until(()=>result(k)===edited(k),key('editable output'));
 await stage(key('copy'),point(copy(k)));
 return {text:input(k).value,result:result(k)};
}
