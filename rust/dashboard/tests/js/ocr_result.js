// Runs inside the real dashboard page with a controlled Local AI bridge.
const quickMode = window.__QUICK_MODE__;
const originalApi = window.pywebview.api;
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
const paste = async (marker) => {
  const data = new DataTransfer();
  data.items.add(new File([marker], 'fixture.png', { type: 'image/png' }));
  document.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data }));
  const expected = 'data:image/png;base64,' + btoa(marker);
  for (let i = 0; i < 40 && node('ocr-preview').getAttribute('src') !== expected; i++) await tick();
  check(node('ocr-preview').getAttribute('src') === expected, 'pasted image did not reach preview');
  return expected;
};
try {
  window.__QUICK_MODE__ = false;
  toolTab('localai');
  node('ai-endpoint').value = 'http://127.0.0.1:11434';
  node('ai-model').value = 'fixture-vision';
  const staleCheck = checkAi();
  await tick();
  check(readiness.length === 1, 'readiness check did not call the local model endpoint');
  node('ai-endpoint').value = 'http://127.0.0.1:18081';
  readiness[0].resolve({ models: ['fixture-vision'] });
  await staleCheck; await tick();
  check(node('ai-status').textContent.includes('Đang kiểm tra'), 'stale readiness response overwrote a changed endpoint');
  node('ai-endpoint').value = 'http://127.0.0.1:11434';
  const emptyCheck = checkAi(); await tick();
  readiness[1].resolve({ models: [] }); await emptyCheck; await tick();
  check(node('ai-status').textContent.includes('chưa có model local'), 'empty model list did not give an actionable diagnosis');
  const readyCheck = checkAi(); await tick();
  readiness[2].resolve({ models: ['fixture-vision'] }); await readyCheck; await tick();
  check(node('ai-status').textContent.includes('Sẵn sàng'), 'reachable selected model was not reported ready');
  toolTab('ocr');
  const first = await paste('first');
  node('ocr-run').click();
  await tick();
  check(requests.length === 1 && requests[0].data.image === first, 'OCR did not receive first image');
  requests[0].resolve({ text: 'first text' });
  await tick();
  check(node('ocr-out').textContent === 'first text' && !translate.disabled && !copy.disabled, 'successful OCR should be actionable');

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
  requests[5].resolve({ error: 'model unavailable' });
  await tick();
  check(node('ocr-out').textContent === 'model unavailable' && copy.disabled && translate.disabled && !node('ocr-run').disabled, 'failed quick OCR did not leave a clear retry state');
  return { ocr_result_lifecycle: true };
} finally {
  window.pywebview.api = originalApi;
  window.__QUICK_MODE__ = quickMode;
}
