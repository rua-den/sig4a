// Actual dashboard renderer, with controlled timestamps and statuses (no credentialed provider).
const native = await api.state();
clearTimeout(timer);
const now = Date.now()/1000;
const longName='Tài khoản '+ 'q'.repeat(92);
const profile = (five,week,extra={}) => ({
  five,week,five_reset:'2035-10-21T10:30:00Z',week_reset:'2035-11-03T18:00:00Z',fetched_at:now,...extra
});
const profiles = {
  urgent:profile(96,83),
  [longName]:profile(83,31,{err:'HTTP 429: ' + 'throttled'.repeat(30),retry_at:now+160}),
  normal:profile(0,9),
  waiting:{}
};
const sample = {...native, cache:{
  ts:now, warn_pct:80, block_pct:95, profiles, busy:['waiting']
}, usage_hooks:{urgent:'blocking',[longName]:'unusable',normal:'ready'}};
const card = name => [...$('accs').querySelectorAll('.acc')].find(x=>x.dataset.n===name);
const meters = el => [...el.querySelectorAll('.quota-meter')];
const equal = (a,b) => Math.abs(a-b) <= 1;
const size = el => {const r=el.getBoundingClientRect();return [r.left,r.top,r.width,r.height]};
const geometry = el => [size(el),...meters(el).map(size)].flat();
const setWidth = async w => {
  await api.set_rect(50,40,Math.round(w*devicePixelRatio),Math.round(620*devicePixelRatio));
  await sleep(250);
  await syncWin();
};
const within = (child,parent) => {
  const a=child.getBoundingClientRect(), b=parent.getBoundingClientRect();
  return a.left>=b.left-1 && a.right<=b.right+1;
};

apply(sample);
toolTab('usage');
check('refresh action remains visible', $('rbtn').textContent === 'Làm mới quota' && !$('rbtn').disabled);
check('refresh spinner and attention are separate', !!card('waiting').querySelector('.spin') && card('waiting').querySelector('.usage-state').textContent === 'Đang cập nhật');
const range = ['limit','warn','ok','empty'];
const colored = [meters(card('urgent'))[0],meters(card('urgent'))[1],meters(card('normal'))[0],meters(card('waiting'))[1]];
check('numeric quotas include a true zero and missing value', colored.map(x=>x.querySelector('.pct').textContent).join('|') === '96%|83%|0%|—');
check('quota labels distinguish all four ranges without relying on color', colored.map(x=>x.dataset.band).join() === range.join() &&
 new Set(colored.map(x=>x.querySelector('.quota-band').textContent)).size === 4);
check('three quota colors visibly differ', new Set(colored.slice(0,3).map(x=>getComputedStyle(x.querySelector('.pct')).color)).size === 3);
check('retry, freshness, full provider error and hook remain available',
 !!card(longName).querySelector('.usage-retry [data-k=until]') &&
 !!card(longName).querySelector('.usage-fresh [data-k=ago]') &&
 card(longName).querySelector('.usage-error').title === profiles[longName].err &&
 card('urgent').querySelector('.usage-hook').dataset.kind === 'blocking');
check('5h and 7d carry separate resets', meters(card('urgent')).map(x=>x.querySelector('.usage-reset').title).join() ===
 [profiles.urgent.five_reset,profiles.urgent.week_reset].join());

for (const width of [760,420]){
 await setWidth(width);
 const a=card(longName), b=card('normal'), el=a.querySelector('.usage-name');
 const [m1,m2]=meters(a).map(x=>x.getBoundingClientRect());
 check(`${width}px viewport has two aligned equal meter columns`,equal(innerWidth,width)&&equal(m1.top,m2.top)&&equal(m1.width,m2.width)&&m2.left>m1.right);
 check(`${width}px long names stay in their slot with the full name available`,el.title===longName&&el.scrollWidth>el.clientWidth&&within(el,a));
 check(`${width}px card contents fit without horizontal document overflow`,document.documentElement.scrollWidth<=innerWidth+1&&
   [...a.querySelectorAll('.usage-identity,.quota-meter,.usage-meta,.usage-error')].every(x=>within(x,a)));
 const oldBox=geometry(a);
 const updated={...sample,cache:{...sample.cache,busy:['waiting',longName],profiles:{...profiles,[longName]:{
   ...profiles[longName],five:84,retry_at:now+62,err:'Timeout'
 }}}};
 apply(updated);
 check(`${width}px updates keep metrics in place and preserve unrelated cards`,card('normal')===b&&
   oldBox.every((v,i)=>equal(v,geometry(card(longName))[i]))&&meters(card(longName))[0].querySelector('.pct').textContent==='84%');
 apply(sample);
}
check('narrow screen uses one card column',new Set([...$('accs').children].map(x=>Math.round(x.getBoundingClientRect().left))).size===1);
toolTab('overview');
const summary=[...$('overview-quota').querySelectorAll('.quota-row')];
check('overview renders the same ranked cards with both meters',summary.map(x=>x.dataset.n).join() === [...$('accs').children].map(x=>x.dataset.n).join()&&
 summary.every(x=>meters(x).length===2&&meters(x).every(m=>within(m,x))));
const mixed={...sample,cache:{...sample.cache,busy:['urgent','waiting'],profiles:{...profiles,urgent:{
  ...profiles.urgent,err:'token hết hạn',status:401
}}}};
apply(mixed);
const urgentSummary=$('overview-quota').querySelector('[data-n="urgent"]');
const flags=urgentSummary.querySelector('.usage-extra');
check('overview keeps concurrent login, stale and refresh reasons with blocking precedence',
 urgentSummary.querySelector('.usage-state').dataset.kind==='blocking'&&flags.getBoundingClientRect().height>0&&
 ['Cần đăng nhập','Dữ liệu cũ','Đang cập nhật'].every(reason=>flags.textContent.includes(reason)&&flags.title.includes(reason)));
apply({...sample,cache:{...sample.cache,profiles:{}}});
check('empty account state renders in both grids',$('accs').textContent.includes('Chưa có tài khoản')&&$('overview-quota').textContent.includes('Chưa có tài khoản'));
apply({...sample,cache:null});
check('first load is different from an empty account list',$('accs').textContent.includes('Đang chờ lần tải quota')&&
 $('overview-quota').textContent.includes('Đang chờ lần tải quota'));
return {R};
