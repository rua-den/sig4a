import { setTimeout as delay } from 'node:timers/promises';

const mode = process.argv[2];
const known = ['ocr', 'copy', 'translate', 'translated', 'translated-copy'];
if (!known.includes(mode)) throw new Error('Invalid read-only probe mode');

async function probe(mode) {
  const wait = async (f, description) => {
    const deadline = performance.now() + 42_000;
    do {
      const result = f();
      if (result) return result;
      await new Promise(resolve => setTimeout(resolve, 100));
    } while (performance.now() < deadline);
    throw new Error(description + ': ' + document.getElementById('ocr-out')?.textContent
      + ' / ' + document.getElementById('ocr-translation-out')?.textContent);
  };
  const el = id => document.getElementById(id);
  const current = () => document.body.classList.contains('ocr-popup')
    && el('tool-ocr')?.classList.contains('on')
    && !el('tool-translation')?.classList.contains('on');
  const copy = el('ocr-copy');
  const viCopy = el('ocr-translation-copy');
  const text = () => el('ocr-out').textContent.trim();
  const vi = () => el('ocr-translation-out').textContent.trim();

  await wait(current, 'Native WebView2 is not in the anchored OCR view');
  function point(control) {
    if (!current() || !control || control.disabled) throw new Error('Expected popup control is unavailable');
    control.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    const b = control.getBoundingClientRect();
    const x = b.left + b.width / 2;
    const y = b.top + b.height / 2;
    if (!b.width || !b.height || !control.contains(document.elementFromPoint(x,y))) {
      throw new Error('Visible pointer target not inside popup viewport');
    }
    return { x: Math.round(x * devicePixelRatio), y: Math.round(y * devicePixelRatio) };
  }
  if (mode === 'copy') return point(copy);
  if (mode === 'translate') return point(document.querySelector('#tool-ocr [data-quick-translate]'));
  if (mode === 'translated-copy') return point(viCopy);
  if (mode === 'translated') {
    await wait(() => current() && !viCopy.disabled && vi(), 'No valid inline Vietnamese result');
    return { text: vi(), ocr: text() };
  }
  // Observe automatic OCR; do not click the Recognize button or invoke a product method.
  await wait(() => current() && !copy.disabled && text(), 'No automatic OCR result');
  const image = el('ocr-preview');
  if (!image.getAttribute('src')?.startsWith('data:image/png;base64,')) {
    throw new Error('Actual native capture was not loaded into the popup');
  }
  await image.decode();
  const canvas = document.createElement('canvas');
  canvas.width = image.naturalWidth;
  canvas.height = image.naturalHeight;
  const ctx = canvas.getContext('2d');
  ctx.drawImage(image, 0, 0);
  const pixels = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
  let dark = 0, white = 0;
  for (let i = 0; i < pixels.length; i += 4) {
    if (pixels[i] < 40 && pixels[i+1] < 40 && pixels[i+2] < 40) dark++;
    if (pixels[i] > 240 && pixels[i+1] > 240 && pixels[i+2] > 240) white++;
  }
  return { width: canvas.width, height: canvas.height, dark, white, text: text() };
}

let tab;
for (let i = 0; i < 60 && !tab; i++) {
  try {
    const tabs = await (await fetch('http://127.0.0.1:18227/json/list', {
      signal: AbortSignal.timeout(1_000),
    })).json();
    tab = tabs.find(x => x.type === 'page' && x.url !== 'about:blank') ?? tabs.find(x => x.type === 'page');
  } catch {}
  if (!tab) await delay(250);
}
if (!tab) throw new Error('No isolated loopback WebView2 target');
const debugUrl = new URL(tab.webSocketDebuggerUrl);
if (debugUrl.protocol !== 'ws:' || debugUrl.hostname !== '127.0.0.1' || debugUrl.port !== '18227') {
  throw new Error('Refusing nonfixture CDP endpoint');
}
const ws = new WebSocket(debugUrl);
try {
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('Opening CDP timed out')), 4_000);
    ws.onopen = () => { clearTimeout(timeout); resolve(); };
    ws.onerror = () => { clearTimeout(timeout); reject(new Error('Cannot open CDP')); };
  });
  const value = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('CDP evaluation timed out')), 52_000);
    ws.onmessage = e => {
      const m = JSON.parse(e.data);
      if (m.id !== 1) return;
      clearTimeout(timer);
      if (m.error || m.result.exceptionDetails) reject(new Error(JSON.stringify(m.error ?? m.result.exceptionDetails)));
      else resolve(m.result.result.value);
    };
    ws.send(JSON.stringify({
      id: 1, method: 'Runtime.evaluate',
      params: { expression: '(' + probe.toString() + ')(' + JSON.stringify(mode) + ')', awaitPromise: true, returnByValue: true }
    }));
  });
  if (!value) throw new Error('Popup probe returned no data');
  console.log(JSON.stringify(value));
} finally {
  ws.close();
}
