// Observes the exact production WebView. Test transfers only points/info;
// Windows mouse input performs every review action. There is no delete-click.
const el=id=>document.getElementById(id);
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const visible=e=>!!e&&e.getClientRects().length>0&&getComputedStyle(e).visibility!=='hidden';
const assert=(ok,msg)=>{if(!ok)throw Error(msg)};
async function wait(f,label){
 const stop=performance.now()+25000;
 while(performance.now()<stop){const v=await f();if(v)return v;await sleep(80);}
 throw Error('Timed out: '+label);
}
function pos(node){
 assert(visible(node)&&!node.disabled,'Target is not available: '+(node?.id||node?.dataset?.id));
 node.scrollIntoView({block:'center',inline:'nearest'});
 const b=node.getBoundingClientRect(),x=b.left+b.width/2,y=b.top+b.height/2;
 assert(b.width>0&&b.height>0&&node.contains(document.elementFromPoint(x,y)),'Target is not hit-testable: '+(node.id||node.dataset.id));
 return {x:Math.round(x*devicePixelRatio),y:Math.round(y*devicePixelRatio)}
}
async function click(stage,node){
 await api.e2e_stage(stage,pos(node));
 await wait(()=>api.e2e_go(stage),'native mouse '+stage);
}
const review=()=>el('clean-review');
const items=()=>[...document.querySelectorAll('#cleanlist input[data-id]')];
const selected=()=>items().filter(e=>e.checked);
const modalButton=()=>el('clean-confirm');
const deleteActions=()=>[el('crecyc'),el('cperm'),modalButton()].filter(Boolean);
const snapshot=()=>api.cleaner_status();
await click('open-cleaner',document.querySelector('.toolnav [data-tool="cleaner"]'));
await wait(()=>visible(el('cscan')),'artifact scan view');
const roots=el('clean-saved-roots');
assert(visible(roots)&&/isolated-fixture/i.test(roots.innerText||roots.value||''),'Saved fixture root is not shown before scanning');
assert(!/outside-scope/i.test(roots.innerText||roots.value||''),'Out-of-scope folder incorrectly shown as a saved root');
assert(deleteActions().every(n=>n.disabled||!visible(n)),'Deletion enabled before first reviewed scan');

await click('run-artifact-scan',el('cscan'));
const scan=await wait(async()=>{const v=await snapshot();return v.phase==='done'&&items().length>=2?v:null},'real fixture scan');
assert((scan.items||[]).some(x=>/node_modules/i.test(x.path))&&scan.items.some(x=>/[\\/]bin(?:$|[\\/])/i.test(x.path)),'Known artifact fixtures were not recognized');
assert(scan.items.every(x=>!/outside-scope|main\.cs/i.test(x.path)),'Normal source or out-of-scope paths appeared in review');
assert(selected().length===0&&deleteActions().every(n=>n.disabled||!visible(n)),'New scan did not start with zero eligible actions');
const chosen=items()[0],row=chosen.closest('label,tr,.cl-row')||chosen.parentElement;
assert(/isolated-fixture/i.test(row.innerText)&&/\d/.test(row.innerText),'Artifact review row does not show full fixture path and size');
await click('select-artifact',chosen);
await wait(()=>selected().length===1&&!el('crecyc').disabled,'one selected fixture');

await click('open-recycle',el('crecyc'));
await wait(()=>visible(review())&&visible(modalButton()),'Recycle confirmation view');
await wait(()=>!modalButton().disabled,'valid backend recycle review ticket');
assert(document.activeElement===el('clean-cancel'),'Recycle review focused a destructive action');
const recycle=review().innerText;
assert(/thùng rác/i.test(recycle)&&/1\s*mục/i.test(recycle)&&/\d+\s*(?:[KMG]B|B)\b/i.test(recycle),'Recycle review missing mode, count or size: '+recycle);
await click('cancel-recycle',el('clean-cancel'));
await wait(()=>!visible(review()),'close Recycle confirmation');
assert(selected().length===1,'Recyling cancellation lost the reviewed selection');

await click('open-permanent',el('cperm'));
await wait(()=>visible(review())&&visible(modalButton()),'permanent confirmation view');
await wait(()=>!modalButton().disabled,'valid backend permanent review ticket');
assert(document.activeElement===el('clean-cancel'),'Permanent review focused a destructive action');
const permanent=review().innerText;
assert(/vĩnh viễn/i.test(permanent)&&/1\s*mục/i.test(permanent)&&recycle!==permanent,'Permanent review was not clearly separate: '+permanent);
await click('cancel-permanent',el('clean-cancel'));
await wait(()=>!visible(review()),'close permanent confirmation');
const afterCancel=await snapshot();
assert(!afterCancel.outcomes?.length&&!afterCancel.cleaned,'A cancelled click reached the deletion worker');

await click('open-duplicates',el('clean-view-dups'));
await wait(()=>visible(el('dscan'))&&!visible(el('cleanlist')),'distinct duplicate view');
assert(deleteActions().every(n=>!visible(n)),'Duplicate view shows artifact deletion controls');
await click('run-duplicate-scan',el('dscan'));
const dups=await wait(async()=>{const d=await api.cleaner_dups_status();return d.phase==='done'&&el('dlist')?.innerText.includes('duplicate-one')?d:null},'real duplicate detection');
assert(dups.groups.some(g=>g.files.some(f=>f.endsWith('duplicate-one.txt'))&&g.files.some(f=>f.endsWith('duplicate-two.txt'))),'Same bytes were not reported together');
assert(dups.groups.every(g=>g.files.every(f=>!/outside-scope/i.test(f))),'Duplicate scanner read the intentionally out-of-scope copy');
assert(el('dlist').querySelectorAll('input, button, [onclick]').length===0&&deleteActions().every(n=>!visible(n)),'Duplicate report is not read-only');
const end=await snapshot();
return {cancelledRecycle:true,cancelledPermanent:true,duplicateGroups:dups.groups.length,duplicatesReadOnly:true,noDeletion:!end.outcomes?.length&&!end.cleaned,artifactCount:items().length,selectedPath:scan.items.find(x=>x.id===chosen.dataset.id)?.path};
