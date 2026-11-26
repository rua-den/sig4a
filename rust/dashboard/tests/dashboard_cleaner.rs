//! E2E cho tab "Dọn ổ" (FEATURE E): cấu hình roots trong sandbox, quét fake tree, tick 1 mục,
//! xác nhận 2 bước rồi dọn. Chạy trên tiến trình dashboard thật (WebView2, chế độ e2e ẩn cửa sổ).
//! Debug/e2e dùng RecordingDeleter => KHÔNG xoá file thật; Rust kiểm tra file vẫn còn trên đĩa.
//! Không quét D:\stu: cleaner_roots bị override trước khi bấm Quét.
//!
//! MỘT test function duy nhất (GUI tests không chạy đồng thời).
#![cfg(any(debug_assertions, feature = "e2e"))]

mod common;
use common::*;
use serde_json::{json, Value};
use std::fs;

const JS: &str = r#"
await waitFor(() => document.querySelectorAll('.acc').length === 1, 8000);
await waitFor(() => $('ver') && $('ver').textContent === 'v2.3.0', 5000);
check('footer shows v2.3.0', $('ver').textContent === 'v2.3.0', $('ver').textContent);
await sleep(1200);
const noScroll = () => { const m = document.querySelector('main'), d = document.documentElement;
  return { ms: m.scrollHeight, mc: m.clientHeight, ds: d.scrollHeight, ih: innerHeight, hs: d.scrollWidth > innerWidth + 1,
           iw: innerWidth, cleanH: $('cleanlist').scrollHeight, cleanC: $('cleanlist').clientHeight }; };
const waitClean = async (f, ms) => { const t = performance.now();
  while (performance.now() - t < ms) { const s = await api.cleaner_status(); if (f(s)) return s; await sleep(50); } return null; };

check('3 tabs trong hộp settings', document.querySelectorAll('.tabs .tab').length === 3, document.querySelectorAll('.tabs .tab').length);
check('panel Dọn ổ ẩn lúc đầu, tab Chung hiện', $('clean').hidden === true && $('gen').hidden === false && !$('t-clean').classList.contains('on'));

// cấu hình từ UI (root đã được test override trong config.json trước khi spawn)
setv('croots', FAKE_ROOTS, 'input'); setv('cdays', '0', 'input'); setv('dmin', '0', 'input');
await saveCleanConfig(); await sleep(350);
let c = await cfgNow();
check('nút lưu ghi cleaner_roots + skip_days qua api.save', Array.isArray(c.cleaner_roots) && c.cleaner_roots[0] === FAKE_ROOTS && c.cleaner_skip_recent_days === 0 && c.cleaner_dups_min_kb === 0,
  JSON.stringify({ roots: c.cleaner_roots, days: c.cleaner_skip_recent_days, min_kb: c.cleaner_dups_min_kb }));
check('ô dmin được init từ config', $('dmin').value === '0', $('dmin').value);

tab('clean'); await sleep(450);
check('footer vẫn v2.3.0 sau khi sang tab Dọn ổ', $('ver').textContent === 'v2.3.0', $('ver').textContent);
check('tab Dọn ổ active, các tab khác ẩn', !$('clean').hidden && $('gen').hidden && $('look').hidden &&
  $('t-clean').classList.contains('on') && !$('t-gen').classList.contains('on'), '');
check('textarea được điền từ config lúc init', $('croots').value === FAKE_ROOTS, $('croots').value);
check('các nút xoá bị disable khi chưa quét', $('cperm').disabled === true && $('crecyc').disabled === true);

$('cscan').click(); await sleep(150);
check('Quét disabled + hiện "đang" khi chạy', $('cscan').disabled === true && $('cscan').textContent.includes('đang'), $('cscan').textContent);
const s0 = await waitClean(x => x.phase === 'done', 12000);
check('quét tới phase done', !!s0, s0 ? s0.phase : 'timeout');
await sleep(500);                                    // để tick poll cuối render xong
const boxes = [...document.querySelectorAll('#cleanlist input[data-id]')];
check('danh sách có ít nhất 2 mục', boxes.length >= 2, boxes.length);
check('tổng > 0', s0 && s0.total > 0, s0 && s0.total);
check('checkbox mặc định CHƯA tick (dry-run)', boxes.every(b => !b.checked), boxes.map(b => b.checked).join(','));
check('kind đúng shape', boxes.length > 0 && [...document.querySelectorAll('#cleanlist .cl-row small')]
  .every(e => ['Bin','Obj','Vs','NodeModules','TestResults'].includes(e.textContent)),
  [...document.querySelectorAll('#cleanlist .cl-row small')].map(e => e.textContent).join(','));
check('path đầy đủ nằm trong title, path rút gọn có ellipsis', [...document.querySelectorAll('#cleanlist .cl-row')]
  .every(r => r.querySelector('span').title.includes('faketree') && getComputedStyle(r.querySelector('span')).textOverflow === 'ellipsis'));
check('dòng trạng thái có scanned/matched/total/elapsed', ['đã quét','khớp','tổng','ms'].every(k => $('cstatus').textContent.includes(k)), $('cstatus').textContent);
check('nút xoá được enable sau khi done', !$('cperm').disabled && !$('crecyc').disabled);

// biểu đồ folder/file nặng: CSS bar ngang, backend đã sort giảm
const chartCheck = (id, data) => {
  const rows = [...document.querySelectorAll('#'+id+' .cbar')];
  check(id+' có ít nhất 1 thanh', rows.length >= 1 && rows.length === data.length, `${rows.length}/${data.length}`);
  const bars = rows.map(r => r.querySelector('i'));
  const widths = bars.map(b => parseFloat(b.style.width));
  const mx = data.reduce((m,e) => Math.max(m, e.size), 0) || 1;
  const want = data.map(e => Math.round(e.size / mx * 100));
  check(id+' thanh lớn nhất width:100%', data.length > 0 && bars[0].style.width === '100%', widths.join(','));
  check(id+' tỉ lệ width đúng size/max', widths.every((w,i) => Math.abs(w - want[i]) <= 1), `${widths.join(',')} vs ${want.join(',')}`);
  check(id+' path đầy đủ trong title, chữ ellipsis', rows.every((r,i) => {
    const sp = r.querySelector('span');
    return sp.title === data[i].path && getComputedStyle(sp).textOverflow === 'ellipsis';
  }), rows.map(r => r.querySelector('span').title).join(' | '));
  check(id+' hiện chữ size', rows.every(r => /\d/.test(r.querySelector('b').textContent)),
    rows.map(r => r.querySelector('b').textContent).join(','));
};
chartCheck('chartFolders', s0.top_folders);
chartCheck('chartFiles', s0.top_files);
check('2 chart nằm ngang khi màn hình rộng', innerWidth <= 640 || $('chartFolders').offsetTop === $('chartFiles').offsetTop,
  `${innerWidth} / ${$('chartFolders').offsetTop} vs ${$('chartFiles').offsetTop}`);

// 460x768 — không được có scrollbar ngoài (pattern của legacy test)
const dpr = devicePixelRatio;
await api.set_rect(150, 90, Math.round(460 * dpr), Math.round(768 * dpr)); await sleep(800);
let g = noScroll();
check('460x768: main/document không cần scrollbar', Math.abs(g.iw - 460) <= 2 && Math.abs(g.ih - 768) <= 2 &&
  g.ms <= g.mc + 1 && g.ds <= g.ih + 1 && !g.hs && g.cleanH <= g.cleanC + 1, JSON.stringify(g));
const cols = getComputedStyle(document.querySelector('.charts')).gridTemplateColumns.split(' ').length;
const cfTop = $('chartFolders').getBoundingClientRect().top, ffTop = $('chartFiles').getBoundingClientRect().top;
check('460px: 2 chart xếp dọc', cols === 1 && ffTop > cfTop, `${cols} / ${cfTop} vs ${ffTop}`);
check('460px: chart không tạo scrollbar ngoài', g.ms <= g.mc + 1 && g.ds <= g.ih + 1, JSON.stringify(g));
check('460px: #ver vẫn hiện v2.3.0, media query không ẩn', $('ver').textContent === 'v2.3.0' &&
  getComputedStyle($('ver')).display !== 'none', $('ver').textContent + ' display=' + getComputedStyle($('ver')).display);

// xác nhận 2 bước, KHÔNG dùng window.confirm
const first = boxes[0];
tick(first, true);
check('tick 1 mục -> "đã chọn 1"', $('ctotal').textContent.includes('đã chọn 1'), $('ctotal').textContent);
const btn = $('cperm');
const beforeArm = (await api.cleaner_status()).outcomes.length;
btn.click();
check('bấm lần 1 chỉ đổi text (chưa gọi API)', btn.textContent.includes('Xác nhận xoá vĩnh viễn 1 mục') &&
  (await api.cleaner_status()).outcomes.length === beforeArm, btn.textContent);
check('text xác nhận gồm số mục', btn.textContent.includes('1 mục'), btn.textContent);
btn.click();
const s1 = await waitClean(x => x.outcomes.length > 0 && x.cleaned >= 1, 12000);
check('bấm lần 2 -> outcomes, cleaned >= 1', !!s1 && s1.cleaned >= 1, JSON.stringify(s1 && { cleaned: s1.cleaned, outcomes: s1.outcomes }));
await sleep(500);
check('#cresult liệt kê outcome (✓)', $('cresult').textContent.includes('✓'), $('cresult').textContent);
const sAfter = await api.cleaner_status();
check('manifest=None -> items rỗng, nút xoá disable', sAfter.items.length === 0 &&
  $('cperm').disabled && $('crecyc').disabled, JSON.stringify({ items: sAfter.items.length, p: $('cperm').disabled, r: $('crecyc').disabled }));
check('nút Quét cho phép quét lại', !$('cscan').disabled && $('cscan').textContent.includes('Quét lại'), $('cscan').textContent);
g = noScroll();
check('sau khi hiện kết quả vẫn không có scrollbar ngoài', g.ms <= g.mc + 1 && g.ds <= g.ih + 1 && !g.hs, JSON.stringify(g));

// poll 1s nhưng applyClean chỉ đụng text/HTML: chiều cao cửa sổ phải đứng yên
const h0 = (await api.rect())[3];
for (let i = 0; i < 5; i++) { applyClean(await api.cleaner_status()); await sleep(200); }
const h1 = (await api.rect())[3];
check('5 tick applyClean trong 1s không resize cửa sổ', h0 === h1, `${h0} -> ${h1}`);

// e2e hook chỉ ghi giá trị return vào file .out (R của check() chỉ được flush qua stage()), nên trả R về đây.
// ---- File trùng: detection-only, poll riêng, KHÔNG checkbox / KHÔNG nút xoá ----
check('subsection File trùng có nhãn "chỉ phát hiện, không xoá"', $('clean').textContent.includes('File trùng') && $('clean').textContent.includes('chỉ phát hiện, không xoá'), '');
check('dlist không có checkbox/nút xoá', $('dlist').querySelectorAll('input,button,[onclick]').length === 0, $('dlist').innerHTML.slice(0,60));
// bridge là Proxy: mọi tênmethod đều trả function, nên gọi thật và expect "unknown method".
let dcleanMissing = false;
try { await api.cleaner_dups_clean([]) } catch(e) { dcleanMissing = /unknown method/.test(String(e)) }
check('không có API cleaner_dups_clean (unknown method)', dcleanMissing, '');
check('nút có nhãn "Tìm file trùng"', $('dscan').textContent === 'Tìm file trùng', $('dscan').textContent);
const waitDups = async (f, ms) => { const t = performance.now();
  while (performance.now() - t < ms) { const s = await api.cleaner_dups_status(); if (f(s)) return s; await sleep(50); } return null; };
$('dscan').click(); await sleep(120);
check('dscan disabled + đang tìm khi chạy', $('dscan').disabled === true && $('dscan').textContent.includes('đang'), $('dscan').textContent);
const ds = await waitDups(x => x.phase === 'done', 15000);
check('dup scan tới phase done', !!ds, ds ? ds.phase : 'timeout');
check('có ít nhất 1 group duplicate', ds && ds.groups.length >= 1, JSON.stringify(ds && { n: ds.groups.length, scanned: ds.scanned, errors: ds.errors }));
check('wasted > 0', ds && ds.groups[0].wasted > 0, ds && ds.groups[0].wasted);
const dpaths = ds ? ds.groups.flatMap(g => g.files) : [];
check('report có path của cả 2 file trùng', ['a.dat','b.dat'].every(n => dpaths.some(p => p.replace(/\//g,'\\').endsWith('dups\\'+n))), JSON.stringify(dpaths));
// file RẤT NHỎ (8 byte) — cleaner_dups_min_kb=0 phải thấy; so NỘI DUNG chứ không chỉ so tên.
const g8 = (ds ? ds.groups : []).filter(g => g.size_each === 8);
check('có đúng 1 group 8 byte', g8.length === 1, JSON.stringify(g8.map(g => ({ n: g.files.length, wasted: g.wasted }))));
const small = g8[0] || { files: [], wasted: -1 };
const sfiles = small.files.map(p => p.replace(/\//g, '\\'));
check('group 8 byte có đúng 2 file', small.files.length === 2, sfiles.join(' | '));
check('wasted = 8', small.wasted === 8, small.wasted);
check('2 file cùng nội dung nằm ở root + stu', sfiles.length === 2 && sfiles.some(p => p.endsWith('\\faketree\\A.txt')) && sfiles.some(p => p.endsWith('\\stu\\A.txt')), sfiles.join(' | '));
check('file cùng tên KHÁC nội dung (other) không vào group', !sfiles.some(p => p.endsWith('\\other\\A.txt')), sfiles.join(' | '));
await sleep(500);
const dg = [...document.querySelectorAll('#dlist .dgrp')];
check('UI hiện đúng số group (tối đa 8)', dg.length >= 1 && dg.length === Math.min(8, ds.groups.length), `${dg.length}/${ds.groups.length}`);
check('group hiện số bản + size_each + wasted', /bản/.test(dg[0].textContent) && dg[0].textContent.includes('MB'), dg[0].textContent);
check('path đầy đủ trong title, ellipsis', [...document.querySelectorAll('#dlist .dp')].every(e => e.title.includes('faketree') && getComputedStyle(e).textOverflow === 'ellipsis'),
  [...document.querySelectorAll('#dlist .dp')].map(e => e.title).join(' | '));
check('dstatus có scanned/skipped/hash/elapsed', ['đã quét','bỏ qua','hash','ms'].every(k => $('dstatus').textContent.includes(k)), $('dstatus').textContent);
check('sau khi có kết quả vẫn KHÔNG có checkbox/xoá cho duplicate', $('dlist').querySelectorAll('input,button,[onclick]').length === 0, '');
g = noScroll();
check('460x768 sau khi hiện duplicate vẫn không scrollbar ngoài', g.ms <= g.mc + 1 && g.ds <= g.ih + 1 && !g.hs, JSON.stringify(g));
const dh0 = (await api.rect())[3];
for (let i = 0; i < 5; i++) { applyDups(await api.cleaner_dups_status()); await sleep(200); }
const dh1 = (await api.rect())[3];
check('5 tick applyDups không resize cửa sổ', dh0 === dh1, `${dh0} -> ${dh1}`);

// banner gốc ổ: chỉ kiểm tra DOM, KHÔNG thực sự quét ổ thật
const savedRoots = $('croots').value;
setv('croots', 'D:\\', 'input');
check('root gốc ổ -> banner hiện, quét không bị chặn', !$('drootwarn').hidden && $('drootwarn').textContent.includes('Đang quét gốc ổ') && !$('dscan').disabled,
  'hidden=' + $('drootwarn').hidden + ' text=' + $('drootwarn').textContent);
setv('croots', savedRoots, 'input');
check('đổi lại root thường -> banner ẩn', $('drootwarn').hidden, 'hidden=' + $('drootwarn').hidden);
// truncated/errors phải hiện rõ trong dstatus
applyDups({phase:'done',groups:[],scanned:123,skipped:0,bytes_hashed:0,elapsed_ms:1,truncated:true,errors:['budget']});
check('dstatus hiện ⚠ bị cắt + lỗi khi truncated', $('dstatus').textContent.includes('⚠ bị cắt, kết quả chưa đầy đủ') && $('dstatus').textContent.includes('đã quét 123') && $('dstatus').textContent.includes('lỗi: budget'), $('dstatus').textContent);

return { ids: boxes.map(b => b.dataset.id), first: first.dataset.id, roots: FAKE_ROOTS, dups: dpaths,
         hasDupsClean: !dcleanMissing, checks: R };
"#;

#[test]
fn dashboard_cleaner_suite() {
    assert!(
        window().is_null(),
        "a dashboard window is already open: close it before running this test"
    );
    let names: Vec<String> = vec!["claude-clean".into()];
    let sb = Sandbox::new(&names);
    sb.write_cache(
        |_i, _n, now| {
            json!({
                "five": 20, "week": 30, "five_reset": "2999-01-01T00:00:00+00:00",
                "week_reset": "2999-02-01T00:00:00+00:00", "fetched_at": now,
                "retry_at": null, "err": null
            })
        },
        &[],
        None,
    );

    // fake tree nằm trong sandbox — test KHÔNG bao giờ quét D:\stu
    let fake = sb.root.path().join("faketree");
    let proj = fake.join("proj");
    fs::create_dir_all(proj.join("bin").join("Release")).unwrap();
    fs::create_dir_all(proj.join("node_modules").join("pkg")).unwrap();
    fs::write(proj.join("proj.csproj"), "<Project Sdk=\"Microsoft.NET.Sdk\"/>").unwrap();
    fs::write(proj.join("bin").join("Release").join("a.dll"), vec![7u8; 4096]).unwrap();
    fs::write(proj.join("package.json"), "{\"name\":\"fake-pkg\"}").unwrap();
    fs::write(proj.join("node_modules").join("pkg").join("i.js"), "export const x=1;").unwrap();

    // 2 file ≥ DEFAULT_MIN_SIZE (1 MiB) cùng nội dung ở 2 path khác nhau: detection-only, KHÔNG xoá.
    let dupdir = fake.join("dups");
    fs::create_dir_all(&dupdir).unwrap();
    let dup_a = dupdir.join("a.dat");
    let dup_b = dupdir.join("b.dat");
    let meg = vec![b'x'; 1 << 20];
    fs::write(&dup_a, &meg).unwrap();
    fs::write(&dup_b, &meg).unwrap();

    // 3 file RẤT NHỎ (8 byte) cùng tên A.txt: 2 cùng nội dung (root + stu), 1 KHÁC nội dung (other).
    // Chứng minh cleaner_dups_min_kb=0 thấy file nhỏ VÀ finder so nội dung thật, không chỉ so tên.
    let small_root = fake.join("A.txt");
    let small_stu = fake.join("stu").join("A.txt");
    let small_other = fake.join("other").join("A.txt");
    for p in [&small_root, &small_stu, &small_other] {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
    }
    let tiny = b"abcdefgh";
    fs::write(&small_root, tiny).unwrap();
    fs::write(&small_stu, tiny).unwrap();
    fs::write(&small_other, b"ABCDEFGH").unwrap();

    // override TRƯỚC khi bấm Quét (và trước cả khi spawn để __INIT__ cũng thấy roots mới)
    let mut cfg = sb.cfg();
    cfg["cleaner_roots"] = json!([fake.to_string_lossy()]);
    cfg["cleaner_skip_recent_days"] = json!(0);
    cfg["cleaner_dups_min_kb"] = json!(0);
    fs::write(sb.data.join("config.json"), cfg.to_string()).unwrap();

    let consts = format!("const FAKE_ROOTS={};", json!(fake.to_string_lossy().to_string()));
    let mut run = Run::start(&sb.local, &sb.home, sb.root.path(), "cleaner", JS, &consts);
    let v = run.finish();
    for (i, c) in v["checks"].as_array().into_iter().flatten().enumerate() {
        run.rust_check(
            &format!("js#{}: {}", i + 1, c[0].as_str().unwrap_or("?")),
            c[1].as_bool().unwrap_or(false),
            c[2].as_str().unwrap_or(""),
        );
    }

    // RecordingDeleter: dọn "thành công" nhưng file vẫn còn nguyên trên đĩa
    let dll = proj.join("bin").join("Release").join("a.dll");
    let js = proj.join("node_modules").join("pkg").join("i.js");
    run.rust_check("RecordingDeleter: a.dll vẫn còn sau khi dọn", dll.exists(), dll.to_string_lossy());
    run.rust_check("RecordingDeleter: i.js vẫn còn sau khi dọn", js.exists(), js.to_string_lossy());

    let cfg = sb.cfg();
    run.rust_check(
        "save() chỉ merge: cleaner_roots vẫn là fake root",
        cfg["cleaner_roots"] == json!([fake.to_string_lossy()]),
        cfg["cleaner_roots"].to_string(),
    );
    run.rust_check(
        "save() không đụng key khác (autostart vẫn false)",
        cfg["autostart"] == json!(false),
        cfg["autostart"].to_string(),
    );
    // id chính là path do backend trả về trong status.items[].id — UI phải gửi nguyên vẹn, không tự dựng.
    // backend canonicalize root (đường dẫn dài), nên so bằng exists() + đuôi thay vì so chuỗi với TempDir.
    let first = v["first"].as_str().unwrap_or("");
    run.rust_check(
        "id gửi lên clean là nguyên path từ status.items[].id (mục bin, lớn nhất)",
        std::path::Path::new(first).exists()
            && first.replace('/', "\\").ends_with("proj\\bin")
            && v["ids"].as_array().is_some_and(|a| a.len() >= 2 && a.iter().all(|x| x.is_string())),
        format!("first={first} ids={}", v["ids"]),
    );

    // File trùng: CHỈ phát hiện — không có API clean, không xoá file nào.
    run.rust_check(
        "không có API cleaner_dups_clean (detection-only)",
        v.get("hasDupsClean") == Some(&json!(false)),
        v["hasDupsClean"].to_string(),
    );
    run.rust_check("duplicate a.dat vẫn còn trên đĩa", dup_a.exists(), dup_a.to_string_lossy());
    run.rust_check("duplicate b.dat vẫn còn trên đĩa", dup_b.exists(), dup_b.to_string_lossy());
    run.rust_check(
        "report trả ít nhất 2 path duplicate",
        v["dups"].as_array().is_some_and(|a| a.len() >= 2 && a.iter().all(Value::is_string)),
        v["dups"].to_string(),
    );
    // 3 file nhỏ vẫn nguyên trên đĩa — duplicate là detection-only.
    run.rust_check("file 8 byte ở root vẫn còn", small_root.exists(), small_root.to_string_lossy());
    run.rust_check("file 8 byte ở stu vẫn còn", small_stu.exists(), small_stu.to_string_lossy());
    run.rust_check("file 8 byte khác nội dung vẫn còn", small_other.exists(), small_other.to_string_lossy());
    run.rust_check(
        "config sandbox có cleaner_dups_min_kb = 0",
        sb.cfg()["cleaner_dups_min_kb"] == json!(0),
        sb.cfg()["cleaner_dups_min_kb"].to_string(),
    );

    report("dashboard cleaner (FEATURE E)", &run.checks);
}
