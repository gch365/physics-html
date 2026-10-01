
const cv = document.getElementById('cv');
const ctx = cv.getContext('2d');
let W=760, H=600;

// ---- 参数（对应 GGB 滑块） ----
const g = 10;
let vb=-5, v0=0, th=37*Math.PI/180, mu=0.5, L=8.2, e=1, r=0.4, rate=0.4;
let showArrow=true, showForce=true, showMark=true, showScr=true, showStrobo=false, showGraph=true;

// ---- 状态（对应 GGB 中 t / v / P / P1 的迭代） ----
let t=0, s=8.2, v=0, sB=8.2, s0=8.2;
let relMin=0, relMax=0, slide=0, off=false;
let hist=[], strobo=[], lastRec=0, lastStro=0;
let playing=false, lastTs=null;

function reset(){
  t=0; s0=e*L; s=s0; sB=s0; v=v0;
  relMin=0; relMax=0; slide=0; off=false;
  hist=[{t:0,v:v0}]; strobo=[s0]; lastRec=0; lastStro=0;
  playing=false; lastTs=null;
  const b=document.getElementById('play'); if(b) b.textContent='▶ 播放';
}

// ---- 物理：加速度 ----
function accel(vv){
  const tanT=Math.tan(th), rel=vb-vv;
  if(Math.abs(rel)<1e-9 && mu>=tanT) return {a:0, stuck:true};
  return {a: -g*Math.sin(th) + Math.sign(rel)*mu*g*Math.cos(th), stuck:false};
}

// ---- 物理：一步迭代（半隐式欧拉，先更新 v 再更新位置，与 GGB 脚本同构） ----
function stepPhysics(dt){
  const A=accel(v);
  if(A.stuck){ v=vb; }
  else{
    let vn=v+A.a*dt;
    if((vb-v)*(vb-vn)<0 && mu>=Math.tan(th)) vn=vb;   // 共速瞬间被静摩擦锁住
    v=vn;
  }
  s  += v*dt;
  sB += vb*dt;
  t  += dt;
  slide += Math.abs(v-vb)*dt;
  const d=s-sB;
  if(d<relMin) relMin=d;
  if(d>relMax) relMax=d;
  if(s<0){ s=0; off=true; }
  if(s>L){ s=L; off=true; }
}

function advance(dtTotal){
  const hstep=0.0005;
  let n=Math.ceil(dtTotal/hstep); if(n<1) n=1; if(n>4000) n=4000;
  const dt=dtTotal/n;
  for(let i=0;i<n;i++){
    if(off) break;
    stepPhysics(dt);
    if(t-lastRec>=0.008){ hist.push({t:t, v:v}); lastRec=t; if(hist.length>8000) hist.shift(); }
    if(t-lastStro>=0.1){ strobo.push(s); lastStro=t; if(strobo.length>400) strobo.shift(); }
  }
  if(off) playing=false;
}

// ---- 画布 ----
// 画布填满舞台剩余高度：先把画布压成 0 量出舞台可用空间，再按可用高度设置画布
function resize(){
  const dpr=window.devicePixelRatio||1;
  const st=cv.parentElement;
  cv.style.height='0px';
  let used=0;
  for(const el of st.children){ if(el!==cv) used+=el.getBoundingClientRect().height; }
  const cs=getComputedStyle(st);
  used+=parseFloat(cs.paddingTop||0)+parseFloat(cs.paddingBottom||0);
  const h=Math.max(420, Math.round(st.clientHeight-used));
  const w=Math.max(320, Math.round(cv.getBoundingClientRect().width||st.clientWidth));
  cv.style.height=h+'px';
  W=w; H=h;
  cv.width=Math.round(w*dpr); cv.height=Math.round(h*dpr);
  ctx.setTransform(dpr,0,0,dpr,0,0);
}
window.addEventListener('resize', ()=>{ resize(); draw(); });

function arrow(x0,y0,x1,y1,color,wd,head){
  const hs=head||8;
  ctx.strokeStyle=color; ctx.fillStyle=color; ctx.lineWidth=wd;
  ctx.beginPath(); ctx.moveTo(x0,y0); ctx.lineTo(x1,y1); ctx.stroke();
  const a=Math.atan2(y1-y0,x1-x0);
  ctx.beginPath(); ctx.moveTo(x1,y1);
  ctx.lineTo(x1-hs*Math.cos(a-0.42), y1-hs*Math.sin(a-0.42));
  ctx.lineTo(x1-hs*Math.cos(a+0.42), y1-hs*Math.sin(a+0.42));
  ctx.closePath(); ctx.fill();
}
function label(txt,x,y,color,font,al,bl){
  ctx.fillStyle=color||'#1e293b'; ctx.font=font||'12px sans-serif';
  ctx.textAlign=al||'center'; ctx.textBaseline=bl||'middle';
  ctx.fillText(txt,x,y);
}

// 视口拟合：把传送带（含两轮）装进给定矩形
function fitScene(x0,y0,w,h){
  const ct=Math.cos(th), st=Math.sin(th);
  const xmin=-r*1.1, xmax=L*ct+r*1.1;
  const ymin=-r*1.35, ymax=L*st+r*1.35;
  const pad=40;
  let sc=Math.min((w-2*pad)/(xmax-xmin), (h-2*pad)/(ymax-ymin));
  if(!isFinite(sc)||sc<=0) sc=10;
  sc=Math.min(sc,110);
  const bottom=y0+h-(h-(ymax-ymin)*sc)/2;
  const ox=x0+(w-(xmax-xmin)*sc)/2 - xmin*sc;
  const oy=bottom + ymin*sc;
  return {sc, X:(x)=>ox+x*sc, Y:(y)=>oy-y*sc};
}

// 传送带是闭环：把"带面弧长坐标" q 映射到世界坐标。
// q∈[0,L] 上表面（沿带向上），[L, L+πr] 绕过上轮，[L+πr, 2L+πr] 下表面（回程面），最后绕过下轮回到起点。
// 这样划痕 / 标记点走出带端后会随带绕过滚轮、在带下方继续显示，不会消失。
function beltFrame(q){
  const ct=Math.cos(th), st=Math.sin(th), nx=-st, ny=ct;
  const Bx=L*ct, By=L*st, P=2*L+2*Math.PI*r, alpha=th+Math.PI/2;
  const qm=((q%P)+P)%P;
  if(qm<=L) return {qm, x:nx*r+qm*ct, y:ny*r+qm*st, inx:-nx, iny:-ny, arc:false};
  if(qm<=L+Math.PI*r){
    const a=alpha-(qm-L)/r, ca=Math.cos(a), sa=Math.sin(a);
    return {qm, x:Bx+r*ca, y:By+r*sa, inx:-ca, iny:-sa, arc:true};
  }
  if(qm<=2*L+Math.PI*r){
    const qb=qm-(L+Math.PI*r);
    return {qm, x:Bx-nx*r-qb*ct, y:By-ny*r-qb*st, inx:nx, iny:ny, arc:false};
  }
  const a=alpha+Math.PI-(qm-(2*L+Math.PI*r))/r, ca=Math.cos(a), sa=Math.sin(a);
  return {qm, x:r*ca, y:r*sa, inx:-ca, iny:-sa, arc:true};
}
function beltPeriod(){ return 2*L+2*Math.PI*r; }

// 沿闭环画一段（自动绕过滚轮）
function strokeBeltSeg(M,qa,qb){
  const len=qb-qa;
  const n=Math.max(2, Math.min(400, Math.ceil(Math.abs(len)/(r*0.12))));
  ctx.beginPath();
  for(let i=0;i<=n;i++){
    const f=beltFrame(qa+len*i/n), px=M.X(f.x), py=M.Y(f.y);
    i?ctx.lineTo(px,py):ctx.moveTo(px,py);
  }
  ctx.stroke();
}

function drawScene(x0,y0,w,h){
  const M=fitScene(x0,y0,w,h), sc=M.sc;
  const ct=Math.cos(th), st=Math.sin(th);
  const ux=ct, uy=st;              // 沿带向上单位矢量
  const nx=-st, ny=ct;             // 带面法向（向上外侧）
  const Bx=L*ct, By=L*st;          // 上轮圆心
  const T=(sq)=>[ nx*r+sq*ux, ny*r+sq*uy ];        // 带上表面距下端 sq 处的世界坐标

  // 皮带带体（两轮 + 中间矩形）
  ctx.fillStyle='#9e9e9e';
  ctx.beginPath(); ctx.arc(M.X(0),M.Y(0),r*sc,0,Math.PI*2); ctx.fill();
  ctx.beginPath(); ctx.arc(M.X(Bx),M.Y(By),r*sc,0,Math.PI*2); ctx.fill();
  ctx.beginPath();
  ctx.moveTo(M.X(nx*r),M.Y(ny*r));
  ctx.lineTo(M.X(Bx+nx*r),M.Y(By+ny*r));
  ctx.lineTo(M.X(Bx-nx*r),M.Y(By-ny*r));
  ctx.lineTo(M.X(-nx*r),M.Y(-ny*r));
  ctx.closePath(); ctx.fill();

  // 轮（浅色）+ 轮辐（转角 −v带/r·t，与 GGB 的 Rotate 一致）
  const phi=-(vb/r)*t;
  [[0,0],[Bx,By]].forEach(c=>{
    const cxp=M.X(c[0]), cyp=M.Y(c[1]), rp=r*sc*0.74;
    ctx.fillStyle='#e0e0e0'; ctx.beginPath(); ctx.arc(cxp,cyp,rp,0,Math.PI*2); ctx.fill();
    ctx.strokeStyle='#9e9e9e'; ctx.lineWidth=1.5; ctx.stroke();
    ctx.strokeStyle='#616161'; ctx.lineWidth=2;
    for(let k=0;k<2;k++){
      const a=phi+k*Math.PI/2;
      ctx.beginPath();
      ctx.moveTo(cxp-rp*Math.cos(a), cyp+rp*Math.sin(a));
      ctx.lineTo(cxp+rp*Math.cos(a), cyp-rp*Math.sin(a));
      ctx.stroke();
    }
    ctx.fillStyle='#424242'; ctx.beginPath(); ctx.arc(cxp,cyp,2.5,0,Math.PI*2); ctx.fill();
  });

  // 带面（上、下）
  const A0=T(0), A1=T(L);
  ctx.strokeStyle='#616161'; ctx.lineWidth=4;
  ctx.beginPath(); ctx.moveTo(M.X(A0[0]),M.Y(A0[1])); ctx.lineTo(M.X(A1[0]),M.Y(A1[1])); ctx.stroke();
  ctx.lineWidth=3;
  ctx.beginPath(); ctx.moveTo(M.X(-nx*r),M.Y(-ny*r)); ctx.lineTo(M.X(Bx-nx*r),M.Y(By-ny*r)); ctx.stroke();

  // 带面花纹（随 v带 移动，直观显示带面运动方向）
  const sp=Math.max(L/16,0.25);
  let ph=((vb*t)%sp+sp)%sp;
  ctx.strokeStyle='#616161'; ctx.lineWidth=2;
  for(let q=ph;q<=beltPeriod();q+=sp){
    const f=beltFrame(q); if(f.arc) continue;        // 花纹只画在上下两段直带上
    ctx.beginPath();
    ctx.moveTo(M.X(f.x),M.Y(f.y));
    ctx.lineTo(M.X(f.x+f.inx*r*0.42),M.Y(f.y+f.iny*r*0.42));
    ctx.stroke();
  }

  // 水平参考线与倾角 θ
  const Q=[st*r,-ct*r];
  const qx=M.X(Q[0]), qy=M.Y(Q[1]);
  ctx.strokeStyle='#94a3b8'; ctx.lineWidth=1; ctx.setLineDash([5,4]);
  ctx.beginPath(); ctx.moveTo(qx,qy); ctx.lineTo(qx+Math.min(3*sc,w*0.35),qy); ctx.stroke(); ctx.setLineDash([]);
  if(th>0.02){
    const ar=Math.min(34, L*sc*0.3);
    ctx.strokeStyle='#2e7d32'; ctx.lineWidth=1.6;
    ctx.beginPath(); ctx.arc(qx,qy,ar,-th,0); ctx.stroke();
    label('θ='+Math.round(th*180/Math.PI)+'°', qx+ar*1.5, qy-ar*0.42, '#2e7d32','12px sans-serif','left');
  }

  // 频闪痕迹（等时间间隔位置，间距变化体现加速度）
  if(showStrobo){
    ctx.strokeStyle='rgba(21,101,192,0.45)'; ctx.lineWidth=1;
    strobo.forEach(q=>{
      const p=T(Math.max(0,Math.min(L,q)));
      ctx.beginPath(); ctx.arc(M.X(p[0]+ux*0.001),M.Y(p[1]),3,0,Math.PI*2); ctx.stroke();
    });
  }

  // 划痕：带上从 relMin 到 relMax 的区间（随带一起移动）
  if(showScr && relMax-relMin>1e-4){
    ctx.strokeStyle='rgba(229,57,53,0.75)'; ctx.lineWidth=6;
    ctx.lineCap='round'; ctx.lineJoin='round';
    strokeBeltSeg(M, sB+relMin, sB+relMax);          // 随带闭环，绕过滚轮到带下方仍可见
    ctx.lineCap='butt'; ctx.lineJoin='miter';
  }

  // 皮带速度箭头（带体内，棕色）
  if(showArrow && Math.abs(vb)>1e-6){
    const mid=[nx*r*0.45+L/2*ux, ny*r*0.45+L/2*uy];
    const len=Math.max(0.35, Math.abs(vb)*0.22)*Math.sign(vb);
    arrow(M.X(mid[0]),M.Y(mid[1]),M.X(mid[0]+len*ux),M.Y(mid[1]+len*uy),'#ffffff',3);
    label('v带='+vb.toFixed(1), M.X(mid[0]+len*0.5*ux)-6, M.Y(mid[1]+len*0.5*uy)+15, '#263238','12px sans-serif');
  }

  // 带上标记点（红，随带匀速运动，用于比较皮带位移）
  if(showMark){
    const f=beltFrame(sB), top=f.qm<=L;              // 转到回程面时半透明，便于区分
    ctx.fillStyle= top?'#e53935':'rgba(229,57,53,0.5)';
    ctx.beginPath(); ctx.arc(M.X(f.x),M.Y(f.y),5,0,Math.PI*2); ctx.fill();
    ctx.strokeStyle= top?'#b71c1c':'rgba(183,28,28,0.6)'; ctx.lineWidth=1; ctx.stroke();
  }

  // 物块（边长 d=L/20 的正方形，贴在带面上）
  const d=L/20;
  const P=T(s);
  const c0=[P[0], P[1]];
  const c1=[P[0]+d*ux, P[1]+d*uy];
  const c2=[c1[0]+d*nx, c1[1]+d*ny];
  const c3=[c0[0]+d*nx, c0[1]+d*ny];
  ctx.beginPath();
  ctx.moveTo(M.X(c0[0]),M.Y(c0[1])); ctx.lineTo(M.X(c1[0]),M.Y(c1[1]));
  ctx.lineTo(M.X(c2[0]),M.Y(c2[1])); ctx.lineTo(M.X(c3[0]),M.Y(c3[1]));
  ctx.closePath();
  ctx.fillStyle='rgba(21,101,192,0.35)'; ctx.fill();
  ctx.strokeStyle='#1565c0'; ctx.lineWidth=2.2; ctx.stroke();
  const cen=[P[0]+d/2*ux+d/2*nx, P[1]+d/2*uy+d/2*ny];
  const cxp=M.X(cen[0]), cyp=M.Y(cen[1]);

  // 物块速度箭头
  if(showArrow && Math.abs(v)>1e-6){
    const len=Math.max(0.4,Math.abs(v)*0.25)*Math.sign(v);
    arrow(cxp,cyp,M.X(cen[0]+len*ux),M.Y(cen[1]+len*uy),'#1565c0',2.6);
    label('v='+v.toFixed(2), M.X(cen[0]+len*ux)+Math.sign(len)*4, M.Y(cen[1]+len*uy)-14, '#1565c0','12px sans-serif', len>0?'left':'right');
  }

  // 受力（沿带方向的重力分量与摩擦力，单位：mg）
  if(showForce){
    const A=accel(v);
    const fg=-Math.sin(th);                                   // 重力分量 / mg
    let ff;                                                    // 摩擦力 / mg
    if(A.stuck) ff=Math.sin(th);
    else ff=Math.sign(vb-v)*mu*Math.cos(th);
    const k=1.5;
    const base=[cen[0]+nx*d*0.15, cen[1]+ny*d*0.15];
    if(Math.abs(fg)>1e-3){
      arrow(M.X(base[0]),M.Y(base[1]),M.X(base[0]+fg*k*ux),M.Y(base[1]+fg*k*uy),'#7e57c2',2.2,7);
      label('mg sinθ', M.X(base[0]+fg*k*ux), M.Y(base[1]+fg*k*uy)+14, '#7e57c2','11px sans-serif');
    }
    if(Math.abs(ff)>1e-3){
      const b2=[cen[0]+nx*d*0.55, cen[1]+ny*d*0.55];
      arrow(M.X(b2[0]),M.Y(b2[1]),M.X(b2[0]+ff*k*ux),M.Y(b2[1]+ff*k*uy),'#2e7d32',2.2,7);
      label(A.stuck?'静摩擦 f':'滑动摩擦 f', M.X(b2[0]+ff*k*ux), M.Y(b2[1]+ff*k*uy)-13, '#2e7d32','11px sans-serif');
    }
  }

  // 端点标注
  label('下端', M.X(A0[0]-ux*0.1)-14, M.Y(A0[1]-uy*0.1)+16, '#475569','12px sans-serif');
  label('上端', M.X(A1[0]),           M.Y(A1[1])-16,        '#475569','12px sans-serif');
}

function drawGraph(x0,y0,w,h){
  const pl=54, pr=24, pt=16, pb=26;
  const gx=x0+pl, gy=y0+pt, gw=w-pl-pr, gh=h-pt-pb;
  ctx.fillStyle='#fff'; ctx.fillRect(x0+6,y0+2,w-12,h-6);
  ctx.strokeStyle='#e3e8ef'; ctx.lineWidth=1; ctx.strokeRect(x0+6,y0+2,w-12,h-6);

  let tMax=Math.max(2, t*1.15);
  let vMax=Math.max(1, Math.abs(vb));
  hist.forEach(p=>{ if(Math.abs(p.v)>vMax) vMax=Math.abs(p.v); });
  vMax*=1.2;
  const X=(tt)=>gx+tt/tMax*gw, Y=(vv)=>gy+gh/2-vv/vMax*(gh/2);

  // 网格
  ctx.strokeStyle='#f1f5f9'; ctx.lineWidth=1;
  for(let i=1;i<=4;i++){
    const yy=gy+gh*i/5; ctx.beginPath(); ctx.moveTo(gx,yy); ctx.lineTo(gx+gw,yy); ctx.stroke();
  }
  // 坐标轴
  ctx.strokeStyle='#334155'; ctx.lineWidth=1.4;
  arrow(gx,Y(0),gx+gw,Y(0),'#334155',1.4,7);
  arrow(gx,gy+gh,gx,gy-2,'#334155',1.4,7);
  label('t/s', gx+gw-2, Y(0)+14, '#334155','12px sans-serif','right');
  label('v/(m/s)', gx+2, gy+4, '#334155','12px sans-serif','left');

  // 刻度
  const dt1=tMax<=3?0.5:(tMax<=8?1:2);
  for(let tt=dt1;tt<=tMax+1e-9;tt+=dt1){
    const xx=X(tt); ctx.strokeStyle='#94a3b8'; ctx.beginPath(); ctx.moveTo(xx,Y(0)-3); ctx.lineTo(xx,Y(0)+3); ctx.stroke();
    label(tt.toFixed(dt1<1?1:0), xx, Y(0)+13, '#64748b','10px sans-serif');
  }
  const dv=vMax<=3?1:2;
  for(let vv=-Math.floor(vMax/dv)*dv; vv<=vMax; vv+=dv){
    if(Math.abs(vv)<1e-9) { label('O', gx-10, Y(0), '#64748b','11px sans-serif'); continue; }
    const yy=Y(vv); ctx.strokeStyle='#94a3b8'; ctx.beginPath(); ctx.moveTo(gx-3,yy); ctx.lineTo(gx+3,yy); ctx.stroke();
    label(String(vv), gx-10, yy, '#64748b','10px sans-serif','right');
  }

  // 皮带 v-t（水平线）
  ctx.strokeStyle='#8d6e63'; ctx.lineWidth=2; ctx.setLineDash([6,4]);
  ctx.beginPath(); ctx.moveTo(gx,Y(vb)); ctx.lineTo(X(Math.max(t,0)),Y(vb)); ctx.stroke(); ctx.setLineDash([]);
  label('v带', X(Math.max(t,0))+2, Y(vb)-9, '#8d6e63','11px sans-serif','left');

  // 物块 v-t
  if(hist.length>1){
    ctx.strokeStyle='#1565c0'; ctx.lineWidth=2.2; ctx.beginPath();
    hist.forEach((p,i)=>{ const xx=X(p.t), yy=Y(p.v); i?ctx.lineTo(xx,yy):ctx.moveTo(xx,yy); });
    ctx.stroke();
    const last=hist[hist.length-1];
    ctx.fillStyle='#1565c0'; ctx.beginPath(); ctx.arc(X(last.t),Y(last.v),3.5,0,Math.PI*2); ctx.fill();
    label('v物', X(last.t)+6, Y(last.v)+10, '#1565c0','11px sans-serif','left');
  }
}

function draw(){
  ctx.clearRect(0,0,W,H);
  const gh = showGraph ? H*0.38 : 0;
  drawScene(0,0,W,H-gh);
  if(showGraph) drawGraph(0,H-gh,W,gh);

  const A=accel(v);
  const x1=s-s0, x2=vb*t, dx=x1-x2, scr=relMax-relMin;
  const set=(id,val)=>{ const el=document.getElementById(id); if(el) el.textContent=val; };
  set('ot',t.toFixed(2)); set('ov',v.toFixed(2)); set('oa',A.a.toFixed(2));
  set('ox1',x1.toFixed(2)); set('ox2',x2.toFixed(2)); set('odx',dx.toFixed(2));
  set('oscr',scr.toFixed(2)); set('oq',(mu*g*Math.cos(th)*slide).toFixed(2));

  const st=document.getElementById('st'), st2=document.getElementById('st2');
  if(st){
    if(off){ st.className='status off'; st.textContent='物块已离开传送带（'+(s<=0?'从下端滑出':'从上端离开')+'）'; }
    else if(A.stuck){ st.className='status on'; st.textContent='与皮带共速：静摩擦 = mg sinθ，a = 0'; }
    else{
      const dir=(vb-v)>0?'带相对物块向上 → 摩擦沿带向上':'带相对物块向下 → 摩擦沿带向下';
      st.className='status off'; st.textContent='相对滑动：'+dir;
    }
  }
  if(st2){
    const tanT=Math.tan(th);
    st2.className = mu>=tanT ? 'status on' : 'status off';
    st2.textContent='μ='+mu.toFixed(2)+(mu>=tanT?' ≥ ':' < ')+'tanθ='+(th>1.5606?'∞':tanT.toFixed(2))
      +(mu>=tanT?'，共速后可随带匀速':'，共速后仍继续下滑加速');
  }
}

// ---- 控件绑定 ----
const CTRL={};
function bind(id,vid,fn,fmt,doReset){
  const el=document.getElementById(id), lb=document.getElementById(vid);
  const upd=()=>{ const x=parseFloat(el.value); fn(x); if(lb) lb.textContent=fmt(x); if(doReset) reset(); draw(); };
  el.addEventListener('input',upd);
  CTRL[id]={el:el, box:el.closest('.ctl'), upd:upd};
  upd();
}
// 直接赋值（供常用值快捷键使用）
function setVal(id,x){
  const c=CTRL[id]; if(!c) return;
  c.el.value=String(x); c.upd();
}
// 加减微调：步长取 .ctl 的 data-st，缺省用 input 的 step
function nudge(id,dir){
  const c=CTRL[id]; if(!c) return;
  const st=parseFloat((c.box&&c.box.dataset.st)||c.el.step)||1;
  const mn=parseFloat(c.el.min), mx=parseFloat(c.el.max);
  let x=parseFloat(c.el.value)+dir*st;
  x=Math.min(mx,Math.max(mn,x));
  const dec=(String(c.el.step).split('.')[1]||'').length;
  setVal(id, parseFloat(x.toFixed(dec)));
}
bind('vb','vbv', x=>vb=x, x=>x.toFixed(1));
bind('v0','v0v', x=>v0=x, x=>x.toFixed(1), true);
bind('th','thv', x=>th=x*Math.PI/180, x=>Math.round(x)+'°', true);
bind('mu','muv', x=>mu=x, x=>x.toFixed(2));
bind('L','Lv',  x=>L=x,  x=>x.toFixed(1), true);
bind('e','ev',  x=>e=x,  x=>(x*L).toFixed(2)+' m', true);
bind('r','rv',  x=>r=x,  x=>x.toFixed(2));
bind('rt','rtv',x=>rate=x, x=>x.toFixed(2)+'×');
const cb=(id,fn)=>{ const el=document.getElementById(id); el.addEventListener('change',ev=>{ fn(ev.target.checked); draw(); }); fn(el.checked); };
cb('cArrow', x=>showArrow=x); cb('cForce', x=>showForce=x); cb('cMark', x=>showMark=x);
cb('cScr', x=>showScr=x);     cb('cStrobo',x=>showStrobo=x); cb('cGraph',x=>showGraph=x);

// 加减按钮（点一次调一步，按住连续调整，适合触摸屏）
let holdT=null, holdI=null;
const stopHold=()=>{ clearTimeout(holdT); clearInterval(holdI); holdT=holdI=null; };
document.querySelectorAll('.step').forEach(b=>{
  const id=b.dataset.id, dir=parseFloat(b.dataset.d);
  b.addEventListener('pointerdown',ev=>{
    ev.preventDefault(); nudge(id,dir); stopHold();
    holdT=setTimeout(()=>{ holdI=setInterval(()=>nudge(id,dir),70); },300);
  });
  ['pointerup','pointerleave','pointercancel'].forEach(e=>b.addEventListener(e,stopHold));
  b.addEventListener('contextmenu',ev=>ev.preventDefault());
});
const playBtn=document.getElementById('play');
playBtn.addEventListener('click',()=>{
  if(off) reset();
  playing=!playing; playBtn.textContent=playing?'⏸ 暂停':'▶ 播放';
  lastTs=null; if(playing) requestAnimationFrame(loop);
});
document.getElementById('reset').addEventListener('click',()=>{ reset(); draw(); });

function loop(ts){
  if(!playing) return;
  if(lastTs==null) lastTs=ts;
  let dt=(ts-lastTs)/1000; lastTs=ts;
  if(dt>0.05) dt=0.05;
  advance(dt*rate);
  draw();
  if(playing) requestAnimationFrame(loop);
  else playBtn.textContent='▶ 播放';
}

resize(); reset(); draw();
