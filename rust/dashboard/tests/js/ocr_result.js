// Runs inside the real dashboard page with a controlled Local AI bridge.
const quickMode = window.__QUICK_MODE__;
const originalApi = window.pywebview.api;
const originalSend = window.__cu_send;
const presentation = [];
window.__cu_send = (method, args) => {
  if (method !== '__quick_presented') return originalSend?.(method, args);
  presentation.push({
    token: args[0], preview: document.getElementById('ocr-preview').getAttribute('src'),
    canCopy: !document.getElementById('ocr-copy').disabled,
    translationHidden: document.getElementById('ocr-inline').hidden,
    anchored: document.body.classList.contains('ocr-popup'),
  });
};
const requests = [];
const readiness = [];
let capture;
window.pywebview.api = new Proxy(originalApi, {
  get(target, key) {
    if (key === 'local_chat') {
      return data => new Promise(resolve => requests.push({ data, resolve }));
    }
    if (key === 'ocr_capture') return () => new Promise(resolve => { capture = resolve; });
    if (key === 'local_models') return (...data) => new Promise(resolve => readiness.push({ data, resolve }));
    return target[key];
  },
});
const check = (condition, message) => { if (!condition) throw new Error(message); };
const node = id => document.getElementById(id);
const translate = document.querySelector('#tool-ocr [data-quick-translate]');
const copy = document.querySelector('#tool-ocr button[onclick*="copyResult"]');
const tick = async () => { await new Promise(resolve => setTimeout(resolve, 0)); };
const assign = (id, value, event = 'input') => {
  node(id).value = value;
  node(id).dispatchEvent(new Event(event, { bubbles: true }));
};
const dispatchImage = file => {
  const data = new DataTransfer();
  data.items.add(file);
  document.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data }));
};
const paste = async (marker) => {
  const canvas = document.createElement('canvas');
  canvas.width = 120; canvas.height = 36;
  const ctx = canvas.getContext('2d'); ctx.font = '20px Arial'; ctx.fillText(marker, 4, 24);
  const expected = canvas.toDataURL('image/png');
  dispatchImage(new File([Uint8Array.from(atob(expected.split(',')[1]), x => x.charCodeAt(0))], 'fixture.png', {type:'image/png'}));
  for (let i = 0; i < 40 && node('ocr-preview').getAttribute('src') !== expected; i++) await tick();
  check(node('ocr-preview').getAttribute('src') === expected, 'pasted image did not reach preview');
  return expected;
};
const state = () => node('ai-status').dataset.state;
try {
  window.__QUICK_MODE__ = false;
  toolTab('translation');
  node('translation-in').value = 'Preserve this draft.';
  assign('ai-endpoint', '');
  await runTool('translation');
  check(!node('translation-recovery').hidden && node('translation-out').textContent.includes('Chưa có endpoint'), 'missing endpoint did not offer recovery');
  node('translation-recovery').click();
  check(node('tool-localai').classList.contains('on'), 'recovery did not open Local AI settings');
  assign('ai-endpoint', 'http://127.0.0.1:11434');
  assign('ai-model', 'fixture-vision');
  const staleCheck = checkAi(); await tick();
  check(readiness.length === 1 && node('model-list').disabled && node('ai-test').disabled, 'setup calls were not serialized');
  await listModels();
  assign('ai-endpoint', 'http://127.0.0.1:18081');
  readiness[0].resolve({ models: ['fixture-vision'] }); await staleCheck;
  check(state() === 'unchecked' && !node('model-list').disabled && !node('ai-test').disabled && !node('ai-check').disabled, 'stale check changed status or locked controls');
  const busyCheck = checkAi(); await tick();
  readiness[1].resolve({ code: 'busy' }); await busyCheck;
  check(state() === 'busy', 'busy misclassified as unreachable');
  const emptyCheck = checkAi(); await tick();
  readiness[2].resolve({ models: [] }); await emptyCheck;
  check(state() === 'missing_model', 'empty list did not give an actionable diagnosis');
  const connected = checkAi(); await tick();
  readiness[3].resolve({ models: ['fixture-vision', 'bad-cloud'] }); await connected;
  check(state() === 'connected' && node('ai-models').options.length === 1, 'discovery must not claim OCR ready or offer a cloud model');
  const test = testAi(); await tick();
  readiness[4].resolve({ models: ['fixture-vision'] }); await tick();
  check(state() === 'testing' && requests[0].data.image.startsWith('data:image/png;base64,') && requests[0].data.task === 'ocr', 'model test did not make an OCR request');
  requests[0].resolve({text:'Close the door.'}); await test;
  check(state() === 'ready', 'successful OCR did not report ready');
  assign('ai-model','new-model');
  check(state() === 'unchecked', 'editing the model left a false ready state');
  node('ai-back').click();
  check(node('tool-translation').classList.contains('on') && node('translation-in').value === 'Preserve this draft.', 'recovery navigation lost entered input');
  requests.length = 0;
  toolTab('ocr');
  const first = await paste('first');
  node('ocr-run').click();
  await tick();
  check(requests.length === 1 && requests[0].data.image === first, 'OCR did not receive first image');
  requests[0].resolve({ text: 'first text' });
  await tick();
  check(node('ocr-out').textContent === 'first text' && !translate.disabled && !copy.disabled, 'successful OCR should be actionable');

  dispatchImage(new File(['no'], 'wrong.gif', {type:'image/gif'}));
  check(node('toast').textContent.includes('Chỉ nhận') && node('ocr-preview').getAttribute('src') === first && !copy.disabled, 'wrong MIME replaced a valid image');
  dispatchImage(new File([new Uint8Array(6 * 1024 * 1024 + 1)], 'oversize.png', {type:'image/png'}));
  check(node('toast').textContent.includes('quá lớn') && node('ocr-preview').getAttribute('src') === first && !copy.disabled, 'oversize replaced a valid image');
  const second = await paste('second');
  check(translate.disabled && copy.disabled && node('ocr-out').textContent !== 'first text', 'replacement image left the previous OCR text actionable');
  node('ocr-run').click();
  await tick();
  check(requests.length === 2 && requests[1].data.image === second, 'OCR did not receive replacement image');
  const third = await paste('third');
  check(node('ocr-run').disabled, 'replacement re-enabled OCR while an old request was pending');
  requests[1].resolve({ text: 'stale text' });
  await tick();
  check(node('ocr-out').textContent !== 'stale text' && translate.disabled && copy.disabled && !node('ocr-run').disabled, 'late OCR response overwrote the current image');

  node('ocr-run').click();
  await tick();
  check(requests.length === 3 && requests[2].data.image === third, 'OCR did not receive current image');
  node('ocr-remove').click();
  requests[2].resolve({ text: 'removed image text' });
  await tick();
  check(node('ocr-out').textContent !== 'removed image text' && node('ocr-run').disabled && translate.disabled && copy.disabled, 'removing an image did not discard its pending result');

  const pendingCapture = captureOcr();
  await tick();
  const fourth = await paste('fourth');
  capture(first);
  await pendingCapture;
  check(node('ocr-preview').getAttribute('src') === fourth, 'late capture replaced a newer pasted image');

  node('ocr-run').click();
  await tick();
  requests[3].resolve({ text: 'current result' });
  await tick();
  translate.click();
  await tick();
  check(requests.length === 5 && requests[4].data.task === 'translation' && requests[4].data.text === 'current result', 'Translate did not receive current OCR text');
  requests[4].resolve({ text: 'translated' });
  await tick();
  check(node('translation-out').textContent === 'translated', 'translation did not complete');
  toolTab('ocr');
  const cancelled = captureOcr();
  await tick();
  capture(null);
  await cancelled;
  check(node('ocr-preview').getAttribute('src') === fourth && !copy.disabled && !translate.disabled, 'cancelled capture discarded the current result');

  window.__QUICK_MODE__ = true;
  const quickCapture = captureOcr();
  await tick();
  capture(first);
  await quickCapture;
  await tick();
  check(requests.length === 6 && requests[5].data.image === first && requests[5].data.task === 'ocr', 'quick capture did not automatically start OCR');
  requests[5].resolve({ code: 'timeout' });
  await tick();
  check(node('ocr-out').textContent.includes('hết thời gian chờ') && !node('ocr-recovery').hidden && copy.disabled && translate.disabled && !node('ocr-run').disabled, 'failed quick OCR did not leave a clear retry state');
  document.body.classList.add('quick');
  node('ocr-recovery').click();
  check(document.body.classList.contains('quick') && node('tool-localai').classList.contains('on'), 'quick recovery exposed unrelated routes');
  node('ai-back').click();
  check(document.activeElement === node('ocr-run'), 'quick OCR recovery did not return focus to visible retry');
  // Actual quick-hotkey callback: a newer crop waits behind the active request
  // and automatically takes over as soon as the previous response is discarded.
  showQuickCapture(first, null, 41);
  check(presentation.length === 1 && presentation[0].token === 41 && presentation[0].anchored &&
    presentation[0].preview === first && !presentation[0].canCopy && presentation[0].translationHidden,
    'first render acknowledgement preceded updating the anchored view');
  await tick();
  check(requests.length === 7 && requests[6].data.image === first, 'anchored capture did not run OCR immediately');
  showQuickCapture(fourth, null, 42);
  check(presentation.at(-1).token === 42 && presentation.at(-1).preview === fourth && !presentation.at(-1).canCopy,
    'replacement render acknowledgement exposed the prior capture');
  await tick();
  check(requests.length === 7 && copy.disabled, 'replacement did not wait behind the active OCR request');
  requests[6].resolve({text:'stale capture A'});
  await tick();
  check(requests.length === 8 && requests[7].data.image === fourth && !node('ocr-out').textContent.includes('stale'), 'newest crop was not queued safely');
  requests[7].resolve({text:'Open the window.'});
  await tick();
  translate.click();
  await tick();
  check(requests.length === 9 && requests[8].data.task === 'translation' && requests[8].data.text === 'Open the window.' && requests[8].data.target === 'vi', 'inline translation request was incorrect');
  check(node('tool-ocr').classList.contains('on') && !node('ocr-inline').hidden && !copy.disabled, 'inline translation hid the OCR result or Copy');
  showQuickCapture(first, null);
  requests[8].resolve({text:'stale translation'});
  await tick();
  check(requests.length === 10 && requests[9].data.image === first && node('ocr-inline').hidden, 'recapture did not invalidate pending translation and re-run OCR');
  requests[9].resolve({text:'Close the door.'});
  await tick();
  translate.click();
  await tick();
  requests[10].resolve({text:'Đóng cửa.'});
  await tick();
  check(node('ocr-out').textContent === 'Close the door.' && node('ocr-translation-out').textContent === 'Đóng cửa.' && !node('ocr-translation-copy').disabled, 'popup did not keep both results available');
  const popupCancel = captureOcr();
  await tick(); capture(null); await popupCancel;
  check(node('ocr-preview').getAttribute('src') === first && node('ocr-out').textContent === 'Close the door.' &&
    node('ocr-translation-out').textContent === 'Đóng cửa.' && !node('ocr-translation-copy').disabled,
    'cancelling recapture discarded prior anchored results');
  const popupButton = captureOcr();
  await tick(); capture({presenting:true}); await popupButton;
  check(node('ocr-preview').getAttribute('src') === first && presentation.length === 2,
    'bridge acknowledgement displayed an image before native presentation');
  node('ocr-out').textContent = 'Edited';
  node('ocr-out').dispatchEvent(new Event('input', {bubbles:true}));
  check(node('ocr-inline').hidden && node('ocr-translation-copy').disabled, 'editing the OCR result retained a stale translation');
  // A previous shortcut or settings readiness check also owns the native AI worker.
  openQuickTool('translation', 'Standalone translation.');
  await tick();
  check(requests.length === 12 && requests[11].data.task === 'translation', 'standalone quick translation did not start');
  showQuickCapture(fourth, null);
  await tick();
  check(requests.length === 12, 'capture competed with the previous quick tool for the native worker');
  requests[11].resolve({text:'previous shortcut'});
  await tick();
  check(requests.length === 13 && requests[12].data.image === fourth, 'capture did not follow the previous shortcut');
  requests[12].resolve({text:'fourth'});
  await tick();
  const checkBeforeCapture = checkAi(); await tick();
  const discovery = readiness.at(-1);
  showQuickCapture(first, null);
  await tick();
  check(requests.length === 13, 'capture competed with Local AI readiness for the native worker');
  discovery.resolve({models:['new-model']});
  await checkBeforeCapture; await tick();
  check(requests.length === 14 && requests[13].data.image === first, 'capture did not follow Local AI readiness');
  requests[13].resolve({text:'first'});
  await tick();
  return { ocr_result_lifecycle: true };
} finally {
  window.pywebview.api = originalApi;
  window.__cu_send = originalSend;
  window.__QUICK_MODE__ = quickMode;
  document.body.classList.remove('ocr-popup');
  document.body.classList.toggle('quick', !!quickMode);
}
