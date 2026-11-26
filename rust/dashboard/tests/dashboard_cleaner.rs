//! E2E cho tab "Dọn ổ" (FEATURE E): cấu hình roots trong sandbox, quét fake tree, tick 1 mục,
//! xác nhận 2 bước rồi dọn. Chạy trên tiến trình dashboard thật (WebView2, chế độ e2e ẩn cửa sổ).
//! Debug/e2e dùng RecordingDeleter => KHÔNG xoá file thật; Rust kiểm tra file vẫn còn trên đĩa.
//! Cleaner roots start empty; the script explicitly saves a disposable TempDir root.
//!
//! MỘT test function duy nhất (GUI tests không chạy đồng thời).
#![cfg(any(debug_assertions, feature = "e2e"))]

mod common;
use common::*;
use serde_json::{json, Value};
use std::fs;

const JS: &str = r#"
const boxes = () => [...document.querySelectorAll('#cleanlist input[data-id]')];
const reviewOpen = () => !$('clean-review').hidden;
const waitClean = async f => {
  await waitFor(() => f(cleanStatus), 12000);
  if(!f(cleanStatus))throw Error('Artifact state did not settle: '+JSON.stringify(cleanStatus));
  return cleanStatus;
};
const readyReview = async mode => {
  $(mode==='recycle'?'crecyc':'cperm').click();
  await waitFor(() => reviewOpen()&&!$('clean-confirm').disabled, 6000);
  if(!reviewOpen()||$('clean-confirm').disabled)throw Error('Review ticket did not become usable: '+mode);
};
const chooseOnly = index => boxes().forEach((b,i)=>{if(b.checked!==(i===index))b.click()});
const beforeSubmit = async () => {
  const s = await api.cleaner_status();
  return s.phase === 'done' && s.outcomes.length === 0;
};

toolTab('cleaner');
check('fresh setup has no implicit roots', !$('croots').value && $('clean-saved-roots').textContent.includes('Chưa chọn'));
check('fresh setup cannot scan either route', $('cscan').disabled && $('dscan').disabled);
setv('croots', FAKE_ROOTS, 'input');
check('unsaved roots cannot scan', $('cscan').disabled && $('dscan').disabled);
await saveCleanConfig();
check('explicit saved scope visible before scanning', $('clean-saved-roots').textContent.includes(FAKE_ROOTS) && !$('cscan').disabled);
check('review actions unavailable before scan', $('crecyc').disabled && $('cperm').disabled);
$('cscan').click();
const s0=await waitClean(s=>s?.phase==='done'&&s.scan_id);
check('scan returns actual scope', s0.scope[0]===FAKE_ROOTS && s0.roots.length===1 && $('clean-scope').textContent.includes('faketree'));
check('full paths, type and size in review table', [...document.querySelectorAll('#cleanlist tbody tr')].every((row,i) =>
 row.querySelector('.clean-path').textContent===s0.items[i].path &&
 row.cells[2].textContent===s0.items[i].kind && row.cells[3].title===s0.items[i].size+' B'));
check('all five artifact types found', ['Bin','Obj','Vs','NodeModules','TestResults'].every(k=>s0.items.some(i=>i.kind===k)));
check('ordinary source not an artefact', !s0.items.some(i=>i.path.includes('ordinary.rs')));
check('initially zero selection and actions disabled', boxes().length>=5 && boxes().every(b=>!b.checked) && $('cperm').disabled && $('crecyc').disabled);

chooseOnly(0);
check('selected bytes are visible', $('ctotal').textContent.includes('Đã chọn 1 mục') && $('ctotal').textContent.includes(String(s0.items[0].size)));
await readyReview('recycle');
check('Recycle is primary; actual reviewed item and count/bytes visible', $('crecyc').classList.contains('p') &&
 $('clean-review-title').textContent.includes('Thùng rác') && $('clean-review-summary').textContent.includes('1 mục') &&
 $('clean-review-summary').textContent.includes(s0.items[0].size+' B') && $('clean-review-paths').textContent.includes(s0.items[0].path));
$('clean-cancel').click();
check('cancel leaves selection intact and does not clean', !reviewOpen() && boxes()[0].checked && await beforeSubmit());
await readyReview('recycle');
boxes()[0].checked=false;boxes()[1].checked=true; // Same count without a change event also must fail at submit.
$('clean-confirm').click();
check('same-count swap cannot submit under a recycled review', !reviewOpen() && await beforeSubmit());
await readyReview('permanent');
check('permanent is distinct and explicitly irreversible', $('clean-review-title').textContent.includes('vĩnh viễn') &&
 $('clean-review-summary').textContent.includes('Không thể khôi phục'));
chooseOnly(0);
check('interactive selection change invalidates permanent review', !reviewOpen() && await beforeSubmit());
await readyReview('permanent');
await readyReview('recycle');
check('switching mode requires its own approval and does not submit', $('clean-review-title').textContent.includes('Thùng rác') && await beforeSubmit());
document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape'}));
check('Escape cancels review without leaving cleaner', !reviewOpen() && document.body.classList.contains('route-cleaner') && await beforeSubmit());
await readyReview('permanent');
toolTab('overview');
check('navigation cancels review', !reviewOpen() && await beforeSubmit());
toolTab('cleaner');await sleep(250);
await readyReview('recycle');
$('clean-view-dups').click();
check('duplicate view cancels artifact approval and hides destructive controls', !reviewOpen() && $('clean-artifacts').hidden && !$('clean-dups').hidden &&
  !$('clean-dups').querySelector('input[data-id],#crecyc,#cperm,#clean-confirm') && await beforeSubmit());
$('clean-view-artifacts').click();
await readyReview('recycle');
$('cscan').click();
const second=await waitClean(s=>s?.phase==='done'&&s.scan_id&&s.scan_id!==s0.scan_id);
check('another scan clears selection and approval even when IDs repeat', !reviewOpen() && boxes().every(b=>!b.checked) &&
  $('cperm').disabled && $('crecyc').disabled && second.items.map(i=>i.id).join()===s0.items.map(i=>i.id).join());
const realStatus=second;
applyClean({...second,truncated:true,matched:30,shown_items:12,manifest_errors:['scan truncated: fixture']});
check('partial result and display limit are both explicit', $('cstatus').textContent.includes('Quét bị cắt') && $('cstatus').textContent.includes('12/30'));
applyClean(realStatus);
chooseOnly(0);
await readyReview('recycle');
setv('cdays','1','input');
check('editing config disables scan and clears confirmation', !reviewOpen() && $('cscan').disabled && $('crecyc').disabled);
setv('cdays','0','input');
await readyReview('recycle');
applyClean({...second,scan_id:null,interrupted:true});
check('interrupted results fail closed', !reviewOpen() && $('crecyc').disabled && $('cstatus').textContent.includes('bị ngắt') && await beforeSubmit());
applyClean(second);
chooseOnly(0);
await readyReview('recycle');
const first=boxes()[0].dataset.id;
$('clean-confirm').click();
const done=await waitClean(s=>s?.phase==='done'&&s.outcomes.length);
check('confirmed operation reports only exact approved id + mode', done.outcomes.length===1 && done.outcomes[0].id===first &&
  done.outcomes[0].ok && done.last_run.mode==='recycle' && $('cresult').textContent.includes('Thành công: 1'));
check('consumed manifest disables selection and actions until new scan', !done.scan_id && boxes().length===0 && $('crecyc').disabled && $('cperm').disabled);
$('clean-view-dups').click();
check('duplicate view remains explicitly detection-only', $('clean-dups').textContent.includes('chỉ phát hiện, không xoá') &&
  $('clean-artifacts').hidden && !$('clean-dups').querySelector('input,#crecyc,#cperm,#clean-confirm'));
$('dscan').click();
await waitFor(() => dupsPhase==='done', 15000);
const ds=await api.cleaner_dups_status();
const dpaths=ds.groups.flatMap(g=>g.files);
check('duplicate report lists real full paths', ds.phase==='done' && ds.scope[0]===FAKE_ROOTS &&
  ['a.dat','b.dat'].every(n=>dpaths.some(p=>p.endsWith(n))) &&
  [...document.querySelectorAll('#dlist .dp')].every(p=>dpaths.includes(p.textContent)));
check('no duplicate selections or destructive controls after results', !document.querySelector('#dlist input,#dlist button,#dlist [onclick]'));
check('duplicates do not enable artifact actions', $('cperm').disabled && $('crecyc').disabled);
let dcleanMissing=false;
try{await api.cleaner_dups_clean([])}catch(e){dcleanMissing=/unknown method/.test(String(e))}
check('there is no duplicate destruction API', dcleanMissing);
applyDups({...ds,truncated:true,errors:['fixture budget']});
check('duplicate truncation is explicit', $('dstatus').textContent.includes('Bị cắt') && $('dstatus').textContent.includes('fixture budget'));

const dpr=devicePixelRatio;
await api.set_rect(150,90,Math.round(420*dpr),Math.round(620*dpr));
await sleep(300);
const root=document.documentElement,main=document.querySelector('main');
main.scrollTop=main.scrollHeight;
check('narrow review has no horizontal overflow and main reaches bottom', root.scrollWidth<=innerWidth+1&&
  Math.abs(main.scrollHeight-main.clientHeight-main.scrollTop)<=1);
return {ids:s0.items.map(i=>i.id),first,dups:dpaths,R};
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
    for name in [".vs", "obj", "TestResults"] {
        let path = proj.join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("cache.bin"), b"fixture artifact").unwrap();
    }
    let ordinary = proj.join("ordinary.rs");
    fs::write(&ordinary, b"fn main() {}").unwrap();

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

    // The real UI starts unconfigured, and only this disposable fixture is saved as a root.
    let checked_files = [&dup_a, &dup_b, &small_root, &small_stu, &small_other, &ordinary];
    let before = checked_files.iter().map(|p| fs::read(p).unwrap()).collect::<Vec<_>>();
    fn checksum(data: &[u8]) -> u64 {
        data.iter().fold(0xcbf29ce484222325u64, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3))
    }

    let consts = format!("const FAKE_ROOTS={};", json!(fake.to_string_lossy().to_string()));
    let mut run = Run::start(&sb.local, &sb.home, sb.root.path(), "cleaner", JS, &consts);
    let v = run.finish();
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

    // Compare fixture checksums and exact bytes; neither a report nor a cancelled review touched them.
    for (path, original) in checked_files.iter().zip(&before) {
        let current = fs::read(path).ok();
        let expected = checksum(original);
        let observed = current.as_deref().map(checksum);
        run.rust_check(
            &format!("pre/post fixture checksum: {}", path.display()),
            observed == Some(expected) && current.as_ref() == Some(original),
            format!("before={expected:016x}; after={observed:?}"),
        );
    }
    run.rust_check("report returned full duplicate paths", v["dups"].as_array().is_some_and(|a| a.len() >= 2 && a.iter().all(Value::is_string)), v["dups"].to_string());

    report("dashboard cleaner (FEATURE E)", &run.checks);
}
