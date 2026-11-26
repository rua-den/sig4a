const sleep=ms=>new Promise(r=>setTimeout(r,ms));
function check(ok,msg){if(!ok)throw Error(msg)}
async function stage(k,v){
 await api.e2e_stage(k,v);
 for(let i=0;i<300;i++){if(await api.e2e_go(k))return;await sleep(100)}
 throw Error('Stage not released: '+k)
}
const main=document.querySelector('main');
const ids=['tool-overview','tool-grammar','tool-translation','tool-ocr','usage-accounts','clean','gen','tool-localai'];
const visible=()=>ids.filter(id=>document.getElementById(id).getClientRects().length>0);
const active=()=>document.querySelector('.toolnav [aria-current]').dataset.tool;
const css=key=>getComputedStyle(document.documentElement).getPropertyValue(key).trim();
for(let i=0;i<40&&document.querySelectorAll('.quota-row').length<2;i++)await sleep(100);
const rows=[...document.querySelectorAll('.quota-row')].map(x=>x.innerText);
const hotkeys=[...document.querySelectorAll('.tile-hotkey')].map(x=>x.innerText);
const theme=['--bg','--panel','--accent'].map(css);
check(active()==='overview'&&document.querySelector('#overview-quota').getBoundingClientRect().top<document.querySelector('.overview').getBoundingClientRect().top,'Tray did not open quota-first Overview');
check(/near-5h.*5h 88%.*gần giới hạn/.test(rows[0])&&/stale-7d.*7d 51%.*dữ liệu cũ/.test(rows[1]),JSON.stringify(rows));
check(hotkeys.join(',')==='Ctrl+Alt+G,Ctrl+Alt+T,Ctrl+Alt+O',JSON.stringify(hotkeys));
check(theme.join(',')==='#0a0a0a,#141414,#fab283'&&getComputedStyle(document.body).fontFamily.includes('Cascadia Mono'),JSON.stringify(theme));
await stage('overview',{rows,hotkeys,theme});
const d=devicePixelRatio;
await api.set_rect(500,100,Math.round(420*d),Math.round(400*d));
await syncWin();await sleep(220);
const link=document.querySelector('.tile[onclick*="localai"]');
main.scrollTop=main.scrollHeight;
const before=main.scrollTop;
link.focus({preventScroll:true});
link.click();
check(before>0&&main.scrollTop===0&&active()==='settings'&&visible().join()==='tool-localai'&&/Cài đặt/.test(link.innerText)&&document.activeElement.dataset.tool==='settings'&&innerWidth===420,'Real scrolled Local AI shortcut failed');
await stage('localai',{before,after:main.scrollTop,active:active(),w:innerWidth,focus:document.activeElement.dataset.tool});
document.querySelector('.title button.x').focus({preventScroll:true});
const primary={overview:'tool-overview',ai:'tool-grammar',usage:'usage-accounts',cleaner:'clean',settings:'gen'};
for(const [route,index] of Object.entries(primary)){
 await stage('tab_'+route,{});
 const f=document.activeElement;
 check(f.dataset.tool===route&&f.matches(':focus-visible')&&getComputedStyle(f).borderTopColor==='rgb(250, 178, 131)','Focus: '+f.outerHTML);
 await stage('enter_'+route,{focused:route});
 check(active()===route&&visible().join()===index&&innerWidth===420&&Math.max(main.scrollWidth-main.clientWidth,document.documentElement.scrollWidth-innerWidth)<=1,'420px routing/overflow failure for '+route+': '+JSON.stringify(visible()));
}
const target={grammar:'tool-grammar',translation:'tool-translation',ocr:'tool-ocr',usage:'usage-accounts',cleaner:'clean',settings:'gen',localai:'tool-localai'};
for(const [k,v] of Object.entries(target)){
 document.querySelector('.toolnav [data-tool=overview]').click();
 document.querySelector('.tile[onclick*="'+k+'"]').click();
 check(visible().join()===v,'Home '+k+': '+visible())
}
for(const k of ['grammar','translation','ocr']){
 document.querySelector('.toolnav [data-tool=ai]').click();
 document.querySelector('.ai-subnav [data-ai='+k+']').click();
 check(visible().join()===target[k],'AI '+k+': '+visible())
}
const accent=document.querySelector('#accent');
accent.value='purple';accent.dispatchEvent(new Event('change',{bubbles:true}));await accentSaving;
check(css('--accent')==='#c7a0ff'&&(await api.state()).cfg.dashboard_accent==='purple','Purple did not persist');
return {nativeKeyboard:Object.keys(primary),directAndAi:'passed',narrowWidth:innerWidth,accent:'#c7a0ff'};
