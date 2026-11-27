// Executes the exact production shim against two WebView documents with recycled request IDs.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const { join } = require("node:path");
const vm = require("node:vm");

function page(seed) {
  const out = [];
  const window = { ipc: { postMessage: m => out.push(m) }, addEventListener() {} };
  const location = { search: "?view=quick-check" };
  vm.runInNewContext(fs.readFileSync(join(__dirname, "../../src/shim.js"), "utf8"), {
    window, location,
    document: { addEventListener() {} },
    crypto: { getRandomValues: bytes => bytes.fill(seed) },
    Uint8Array, setTimeout: () => 0, clearTimeout() {},
  });
  function wire() {
    const message = out.shift();
    assert.ok(message.startsWith("sig4a-qc:"));
    return JSON.parse(message.slice(9));
  }
  return { window, location, wire };
}

(async () => {
  const a = page(1), b = page(2), scope = "00000000000000020000000000000002";
  a.window.pywebview.api.qc_translation(scope);
  const ar = a.wire();
  let settled = false;
  const second = b.window.pywebview.api.qc_translation(scope).then(v => { settled = true; return v; });
  const br = b.wire();
  assert.equal(ar.id, br.id, "same request id in the reload must not be globally unique");
  assert.notEqual(ar.a[0], br.a[0], "document witnesses differ");
  b.window.__cu_resolve(br.id, true, {preview:"generic stale"});
  b.window.__cu_qc_resolve(ar.id, ar.a[0], scope.slice(0,16), {preview:"stale document"});
  b.window.__cu_qc_resolve(br.id, br.a[0], "0000000000000001", {preview:"stale route"});
  b.location.search = "";
  b.window.__cu_qc_resolve(br.id, br.a[0], scope.slice(0,16), {preview:"off page"});
  await Promise.resolve();
  assert.equal(settled, false, "generic, wrong source and stale epoch cannot settle work");
  b.location.search = "?view=quick-check";
  b.window.__cu_qc_resolve(br.id, br.a[0], scope.slice(0,16), {preview:"fresh"});
  assert.deepEqual(await second, {preview:"fresh"});
  const cancelled = b.window.pywebview.api.qc_translation(scope);
  b.window.__cu_qc_forget();
  assert.equal(await cancelled, null);
  console.log("Scoped resolver rejects recycled ID after reload, old epoch and off-page results.");
})().catch(e => { console.error(e); process.exitCode = 1; });
