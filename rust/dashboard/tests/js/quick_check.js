const buttonsReady=()=>!$('qc-refresh').disabled&&!$('qc-translation').disabled&&!$('qc-grammar').disabled;
const get=(key)=>$('qc-'+key+'-status').textContent;
const open=async()=>{
 toolTab('settings');
 $('t-quick-check-settings').click();
 return waitFor(buttonsReady,3000);
};
check('Quick Check is a Settings tab, not the default or a Tool Box route',
 $('quick-check-settings').hidden&&!document.querySelector('.toolnav [data-tool="quick-check-settings"]'));
const offPage=await api.qc_enter();
check('native rejects direct entry from the ordinary committed dashboard route',
 offPage?.code==='view_lost'&&location.search==='');
check('native view opens without automatically checking or sampling',await open()&&location.search==='?view=quick-check'&&
 get('refresh').includes('Chưa kiểm tra')&&get('translation').includes('Chưa thử')&&get('grammar').includes('Chưa thử'));
check('cached cards distinguish saved setup and unverified execution',
 $('qc-ai-config').textContent.includes('gemma4:e2b')&&get('notify').includes('Đang bật')&&
 $('qc-open-hooks').textContent.includes('Hook CLI')&&
 $('qc-shortcut-status').previousElementSibling.textContent.includes('chưa thử tổ hợp'));
check('each fixed status has an accessibility group separate from model text',
 ['translation','grammar'].every(key=>{
  const scope=$('qc-'+key+'-status-scope');
  return scope.getAttribute('role')==='group'&&scope.contains($('qc-'+key+'-status'))&&
   !scope.contains($('qc-'+key+'-output'))&&scope.nextElementSibling===$('qc-'+key+'-output');
 }));
await stage('opened-passive');
const wrong=await api.qc_refresh('00000000000000000000000000000000');
check('wrong native view scope refuses even metadata',wrong?.code==='view_lost');
$('qc-refresh').click();
check('real unauthorized endpoint Refresh refuses before network',await waitFor(()=>get('ai').includes('Ollama chính thức')&&buttonsReady(),2500));
$('qc-translation').click();
check('real unauthorized endpoint Translation refuses with no output',
 await waitFor(()=>get('translation').includes('Ollama chính thức')&&$('qc-translation-output').hidden&&buttonsReady(),2500));
$('qc-grammar').click();
check('real unauthorized endpoint Grammar refuses with no output',
 await waitFor(()=>get('grammar').includes('Ollama chính thức')&&$('qc-grammar-output').hidden&&buttonsReady(),2500));
await stage('refused-live-transport');
// Size after the read-only baseline so geometry persistence isn't mistaken for a Quick Check side effect.
const qcRect=await api.rect(), qcDpr=window.devicePixelRatio||1;
await api.set_rect(qcRect[0],qcRect[1],Math.round(420*qcDpr),Math.round(700*qcDpr));
await syncWin();
const narrow=await waitFor(()=>Math.abs(innerWidth-420)<=2,2500);
const quickTab=$('t-quick-check-settings');quickTab.focus({preventScroll:true});
const quickBounds=quickTab.getBoundingClientRect();
check('Quick Check tab remains readable and keyboard focusable at 420px',narrow&&document.activeElement===quickTab&&
 quickBounds.width>0&&quickBounds.left>=0&&quickBounds.right<=innerWidth&&
 document.documentElement.scrollWidth<=innerWidth,JSON.stringify({width:innerWidth,left:quickBounds.left,right:quickBounds.right}));
check('debug fixture requires explicit script activation',await api.__e2e_quick_check_fixture()===true);
$('qc-refresh').click();
check('metadata fixture never marks a text sample passed',await waitFor(()=>get('ai').includes('Chưa chạy mẫu')&&buttonsReady(),3000)&&$('qc-translation-output').hidden);
$('qc-translation').click();
check('second action disabled during first admitted check',$('qc-grammar').disabled&&$('qc-refresh').disabled);
check('one compiled sample can render match evidence',await waitFor(()=>get('translation').includes('Mẫu khớp')&&
 $('qc-translation-output').textContent==='Robot có hai chiếc hộp màu xanh dương.'&&buttonsReady(),3000));
$('qc-grammar').click();
check('mismatch is not a quality fail and renders output as literal text',await waitFor(()=>get('grammar').includes('mẫu chưa khớp')&&
 $('qc-grammar-output').textContent.startsWith('<b>')&&!$('qc-grammar-output').firstElementChild&&buttonsReady(),3000));
const old=qcNonce;
const pending=api.qc_grammar(old);
await sleep(100);
$('qc-open-alerts').click();
check('alerts link focuses the existing tab without sending a test',$('quick-check-settings').hidden&&!$('alerts').hidden&&document.activeElement===$('t-alerts'));
check('leaving cancels the previous scope reply without preview',(await pending)==null);
const rejected=await api.qc_refresh(old);
check('native refuses the old scope and new entry outside the reserved committed route',
 rejected?.code==='view_lost'&&(await api.qc_enter())?.code==='view_lost'&&location.search==='');
check('re-entry resets evidence and never auto-runs an earlier sample',await open()&&get('grammar').includes('Chưa thử')&&$('qc-grammar-output').hidden);
$('qc-open-hooks').click();
check('Hook CLI link selects and focuses existing tab',!$('hooks').hidden&&document.activeElement===$('t-hooks'));
await open();$('qc-open-shortcuts').click();
check('Phím tắt link selects and focuses existing tab',!$('shortcuts').hidden&&document.activeElement===$('t-shortcuts'));
await open();$('qc-open-live').click();
check('Live link opens without capture or device enumeration',$('tool-live').classList.contains('on')&&$('live-lamp').dataset.listening==='false');
await open();$('qc-open-ai').click();
check('Local AI link opens existing configuration and focuses heading',$('tool-localai').classList.contains('on')&&document.activeElement===$('tool-localai').querySelector('.lbl'));
return {R};
