const original = window.pywebview.api;
const get = id => document.getElementById(id);
const check = (condition, message) => { if (!condition) throw Error(message); };
let status = {phase:'idle'}, starts = 0, cancels = 0, saves = 0;
window.pywebview.api = new Proxy(original, { get(target, key) {
 if(key === 'local_ai_setup_status') return async () => status;
 if(key === 'local_ai_setup_start') return async () => {
  starts++;
  status = starts === 1 ? {phase:'installing',active:true,can_cancel:true}
     : starts === 2 ? {phase:'failed',code:'install_failed',exit_code:12}
     : {phase:'pulling',active:true,can_cancel:true};
  return {accepted:true};
 };
 if(key === 'local_ai_setup_cancel') return async () => { cancels++;status={phase:'cancelled'};return {accepted:true}; };
 if(key === 'local_ai_settings') return async () => null;
 if(key === 'local_ai_diagnostics') return async () => ({server:'reachable',models:['old-model'],model_status:'listed',ocr:'unknown'});
 if(key === 'save') return async patch => { saves++;return patch.local_ai_model==='gemma4:e2b' && patch.local_ai_prompt_ocr==='Custom OCR prompt'; };
 return target[key];
}});
try {
 applySavedAiSettings(savedAiConfig({local_ai_backend:'llama_cpp',local_ai_endpoint:'http://127.0.0.1:8081',local_ai_model:'old-model',local_ai_prompt_ocr:'Custom OCR prompt'}));
 toolTab('localai');
 await pollAiSetup();
 await checkAi();
 check(starts===0 && saves===0,'navigating or checking must not install or save');
 check(get('ai-setup').textContent.includes('4,6 GB')&&get('ai-setup').textContent.includes('Microsoft WinGet'),'missing pre-click disclosures');
 await startAiSetup();
 await pollAiSetup();
 check(starts===1&&get('ai-setup-status').dataset.phase==='installing'&&!get('ai-setup-cancel').hidden,'install progress and cancellation not visible');
 await startAiSetup();
 check(starts===1,'active setup was duplicated');
 await cancelAiSetup();
 await pollAiSetup();
 check(cancels===1&&get('ai-setup-status').dataset.phase==='cancelled','cancelled setup did not stop at caller');
 await startAiSetup();
 await pollAiSetup();
 check(starts===2&&get('ai-setup-status').textContent.includes('0xC')&&!get('ai-setup-start').disabled,'failure did not give diagnostics and retry');
 await startAiSetup();
 await pollAiSetup();
 check(starts===3&&get('ai-setup-status').dataset.phase==='pulling','model download phase not displayed');
 status={phase:'ready',started_server:true};
 await pollAiSetup();
 check(saves===0&&get('ai-model').value==='old-model'&&get('ai-backend').value==='llama_cpp'&&get('ai-prompt-ocr').value==='Custom OCR prompt','setup unexpectedly modified config');
 check(!get('ai-setup-apply').disabled&&!get('ai-setup-apply').hidden,'preset not offered explicitly');
 await applyAiSetup();
 check(saves===0&&get('ai-model').value==='gemma4:e2b'&&get('ai-prompt-ocr').value==='Custom OCR prompt','apply must stage only and preserve prompts');
 await saveAi();
 check(saves===1&&get('ai-save-status').textContent.includes('Đã lưu Local AI'),'normal save did not persist staged preset');
 await startAiSetup();
 await pollAiSetup();
 get('ai-model').value='unsaved-new-choice';
 get('ai-model').dispatchEvent(new Event('input',{bubbles:true}));
 status={phase:'ready'};
 await pollAiSetup();
 await applyAiSetup();
 check(get('ai-setup-apply').disabled&&get('ai-model').value==='unsaved-new-choice'&&saves===1,'setup overwrote a concurrent draft');
 return {local_ai_setup:true};
} finally {
 clearTimeout(setupTimer);
 window.pywebview.api=original;
}
