await until(()=>loaded&&document.querySelector('.toolnav'),'visible dashboard');
await stage('open-usage',point(document.querySelector('.toolnav [data-tool="usage"]')));
const rows=await until(()=>{
 const a=[...document.querySelectorAll('#accs .acc[data-n]')];
 return a.length===10?a:null;
},'ten profile rows');
const rowInfo=rows.map(e=>({name:e.dataset.n,state:e.dataset.attention,labels:e.innerText,hook:e.querySelector('.usage-hook')?.innerText,stateLabel:e.querySelector('.usage-state')?.innerText}));
const r=rows[0].getBoundingClientRect();
assert(rows[0].dataset.n==='active-hook','Most urgent active-hook profile is not first: '+JSON.stringify(rowInfo));
assert(r.top>=0&&r.top<innerHeight,'Priority profile is not visible without scrolling');
assert(/chặn|block/i.test(rowInfo[0].hook||''),'Actual configured hook did not say it would block on submit');
const byName=n=>rowInfo.find(x=>x.name===n);
assert(byName('limit-without-hook')&&!/đang chặn|sẽ chặn|would block/i.test(byName('limit-without-hook').hook||''),'Usage alone claimed a missing hook was blocking');
assert(byName('login-needed')?.state==='login'&&byName('stale-quota')?.state==='stale','Stale and login states were not explicit');
assert(byName('refreshing')?.state==='refreshing','Refreshing profile did not have its own state');
assert(rows[0].querySelector('[data-window="5h"] .usage-reset')&&rows[0].querySelector('[data-window="7d"] .usage-reset'),'The two reset windows were not separately rendered');
assert(/5h/i.test(rowInfo[0].labels)&&/7d/i.test(rowInfo[0].labels),'Both reset windows were not visible');
await stage('usage-priority',{rowInfo,visible:true});

await stage('open-ai',point(document.querySelector('.toolnav [data-tool="ai"]')));
await until(()=>shown('grammar'),'dashboard grammar');
const grammar=await textFlow('grammar',{prefix:'dg-',manual:true});
await stage('open-translation',point(document.querySelector('.ai-subnav [data-ai="translation"]')));
await until(()=>shown('translation'),'dashboard translation');
const translation=await textFlow('translation',{prefix:'dt-',manual:true});

await stage('open-settings',point(document.querySelector('.toolnav [data-tool="settings"]')));
await until(()=>visible(el('t-alerts'))&&visible(el('t-hooks'))&&visible(el('t-look')),'separate setting panels');
await stage('alerts-tab',point(el('t-alerts')));
await stage('hooks-tab',point(el('t-hooks')));
await stage('appearance-tab',point(el('t-look')));
const legacyLabel=el('wlabel')?.value;
assert(legacyLabel==='short'&&el('wfont').value==='124'&&el('wopac').value==='81','Legacy widget preferences were not rendered');
await stage('change-accent',point(el('accent')));
await until(()=>el('accent').value==='purple','native accent change');
return {tenProfiles:true,actualHook:true,grammar,translation,legacyLabel,accent:el('accent').value};
