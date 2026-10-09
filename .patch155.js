// ===== 增强补丁（会被拼进 155 原脚本的间接 eval，因此可直接访问脚本内部的 const/let）=====
// 设计原则：① 任何一项挂了都不能拖垮其他项 → 全部用 __safe 包起来；
//          ② 事件绑定放最后；③ 所有 DOM 引用都做 null 保护（曾经因为漏了一个元素 id
//             导致 toggleEq(false) 抛错，整段补丁在注册 drawEquipotentials 之前就挂了）。
var __P$ = function(id){ return document.getElementById(id); };
var __safe = function(f){ try { return f(); } catch (e) { if (window.console && console.warn) console.warn('[155补丁]', e); } };

/* ---------- 一、等差等势面（marching squares 提取等值线） ---------- */
var potAt = function(x, y){
  var v = 0;
  for (var i = 0; i < sourceCharges.length; i++){
    var sc = sourceCharges[i];
    if (!sc.q) continue;
    var dx = x - sc.x, dy = y - sc.y, r = Math.sqrt(dx*dx + dy*dy);
    if (r < 4) return NaN;               // 电荷正中心那一点：标记为无效，避免画出寄生小圈
    v += K * sc.q / r;
  }
  return v;
};
var eqBox = __P$('show-eq'), eqSteps = __P$('eq-steps'), eqBtn = __P$('eq-btn'), eqVal = __P$('eq-val');
var eqOn = function(){ return !!(eqBox && eqBox.checked); };
var eqN = function(){ return parseInt(eqSteps && eqSteps.value, 10) || 14; };
var toggleEq = function(force){
  var on = (force === undefined) ? !eqOn() : !!force;
  if (eqBox) eqBox.checked = on;
  if (eqBtn) eqBtn.textContent = on ? '✅ 等差等势面：开（点击关闭）' : '📐 一键等差等势面（点击显示/隐藏）';
  return on;
};
var syncEqVal = function(){ if (eqVal) eqVal.textContent = eqSteps ? eqSteps.value : '14'; };

// 等势面只跟「场源电荷位置/电量 + 画布尺寸 + 层数」有关，本身是静态的 → 缓存到离屏画布：
// 每帧只 drawImage 一次，网格因此可以放心加密到 step=6 而不掉帧。
var eqCv = document.createElement('canvas'), eqCtx = eqCv.getContext('2d');
var eqSig = function(){
  var s = width + 'x' + height + '|' + eqN();
  for (var i = 0; i < sourceCharges.length; i++){
    var sc = sourceCharges[i];
    s += '|' + sc.x + ',' + sc.y + ',' + sc.q;
  }
  return s;
};
var eqLastSig = '', eqDirty = true;

// 关键：截断值必须取「离电荷足够近」处的电势，否则最内圈等势面会跑到 r=200 以外，
// 看上去就像"靠近电荷的地方没有等势面"。这里用 Rmin=45px：最内圈等势线画在离电荷约 45px 处。
var RMIN_EQ = 45;

var renderEqui = function(){
  if (!width || !height) return;
  var qmax = 0;
  for (var i = 0; i < sourceCharges.length; i++) qmax = Math.max(qmax, Math.abs(sourceCharges[i].q));
  if (!qmax){ /* 无场源时给个提示，别让老师以为开关坏了 */ }
  var N = eqN();
  var Vcap = qmax ? K * qmax / RMIN_EQ : 0;
  var dv = Vcap / N;
  var step = 6;      // 网格 6px：单电荷最内圈约 60 段折线，圆度误差 < 0.2px
                     // （原 13px 只有 28 段；且近场 clamp 会把最内圈抖成 ±2.3px 的怪形状）
  eqCv.width = width; eqCv.height = height;        // 赋值即清空，正好每帧重画
  var c = eqCtx;
  var cols = Math.max(2, Math.floor(width / step) + 1);
  var rows = Math.max(2, Math.floor(height / step) + 1);
  var g = new Float64Array(cols * rows);
  var j, i2;
  for (j = 0; j < rows; j++){
    var y = j * step;
    for (i2 = 0; i2 < cols; i2++){
      var v = potAt(i2 * step, y);
      if (v === v && isFinite(v)) g[j * cols + i2] = v; else g[j * cols + i2] = NaN;
    }
  }
  c.clearRect(0, 0, width, height);
  c.lineWidth = 1.2; c.font = '10px Arial'; c.textAlign = 'left';
  var nseg = 0;
  for (var n = -N; n <= N; n++){
    var tv = n * dv;
    var edge = (n === N || n === -N);      // 最内/最外一圈画实线加粗，一眼就能看出是不是正圆
    c.setLineDash((tv === 0 || edge) ? [] : [5, 4]);
    c.lineWidth = edge ? 1.8 : 1.2;
    c.strokeStyle =  tv > 0 ? 'rgba(214,51,51,0.5)' : (tv < 0 ? 'rgba(33,110,220,0.5)' : 'rgba(20,140,80,0.8)');
    c.beginPath();
    var first = null;
    for (var j2 = 0; j2 < rows - 1; j2++){
      var yA = j2 * step, yB = yA + step, r0 = j2 * cols, r1 = r0 + cols;
      for (var i3 = 0; i3 < cols - 1; i3++){
        var v00 = g[r0 + i3], v10 = g[r0 + i3 + 1], v01 = g[r1 + i3], v11 = g[r1 + i3 + 1];
        if (v00 !== v00 || v10 !== v10 || v01 !== v01 || v11 !== v11) continue;  // 电荷内部跳过
        var k = 0;
        if (v00 > tv) k |= 1; if (v10 > tv) k |= 2; if (v11 > tv) k |= 4; if (v01 > tv) k |= 8;
        if (k === 0 || k === 15) continue;
        var x0 = i3 * step, x1 = x0 + step;
        var eTx = x0 + step * (tv - v00) / (v10 - v00), eTy = yA;
        var eRx = x1,                                 eRy = yA + step * (tv - v10) / (v11 - v10);
        var eBx = x0 + step * (tv - v01) / (v11 - v01), eBy = yB;
        var eLx = x0,                                 eLy = yA + step * (tv - v00) / (v01 - v00);
        var ax, ay, bx, by;
        switch (k){
          case 1: case 14: ax = eLx; ay = eLy; bx = eTx; by = eTy; break;
          case 2: case 13: ax = eTx; ay = eTy; bx = eRx; by = eRy; break;
          case 3: case 12: ax = eLx; ay = eLy; bx = eRx; by = eRy; break;
          case 4: case 11: ax = eRx; ay = eRy; bx = eBx; by = eBy; break;
          case 6: case 9:  ax = eTx; ay = eTy; bx = eBx; by = eBy; break;
          case 7: case 8:  ax = eBx; ay = eBy; bx = eLx; by = eLy; break;
          case 5:  ax = eLx; ay = eLy; bx = eBx; by = eBy; c.moveTo(eTx, eTy); c.lineTo(eRx, eRy); break;
          case 10: ax = eTx; ay = eTy; bx = eLx; by = eLy; c.moveTo(eRx, eRy); c.lineTo(eBx, eBy); break;
          default: continue;
        }
        c.moveTo(ax, ay); c.lineTo(bx, by); nseg++;
        if (!first) first = { x: (ax + bx) / 2, y: (ay + by) / 2 };
      }
    }
    c.stroke();
    if (first && n % 3 === 0){
      c.setLineDash([]);
      c.fillStyle = tv > 0 ? 'rgba(214,51,51,0.95)' : (tv < 0 ? 'rgba(33,110,220,0.95)' : 'rgba(20,120,70,0.95)');
      c.fillText((tv > 0 ? '+' : '') + tv.toFixed(0), first.x + 4, first.y + 3);
      c.setLineDash([5, 4]);
    }
  }
  c.setLineDash([]);
  if (!nseg){   // 一层都没画出来：通常是画布内电势变化范围小于层间电势差，给个明确提示
    c.font = '13px Arial'; c.textAlign = 'center';
    c.fillStyle = 'rgba(110,110,110,0.95)';
    c.fillText(qmax ? '当前电荷量下画布内画不出等势面（试着加大电荷量或减少层数）' : '场源电荷为 0，画布内无电势可画',
               width / 2, height - 16);
  }
};

var drawEquipotentials = function(){
  if (!eqOn()) return;
  var sg = eqSig();
  if (sg !== eqLastSig){ eqLastSig = sg; eqDirty = true; }
  if (eqDirty){
    try { renderEqui(); } catch (e) { if (window.console && console.warn) console.warn('[155补丁] 等势面渲染', e); }
    eqDirty = false;
  }
  ctx.save();
  ctx.setTransform(1, 0, 0, 1, 0, 0);   // 画布宽高 = offsetWidth/offsetHeight，无 DPR 缩放，1:1 贴图
  ctx.drawImage(eqCv, 0, 0);
  ctx.restore();
};

/* ---------- 二、试探电荷初速度 ---------- */
var v0r = __P$('v0-range'), thr = __P$('th-range');
var curV = function(){ return parseFloat(v0r && v0r.value) || 0; };
var curT = function(){ return parseFloat(thr && thr.value) || 0; };

// 待放入试探电荷的电量（正负 + 大小）。原脚本 add-test 硬编码 q:1，
// 老师没法学"放正电还是放负电" → 这里在放入前先给一个待定 q，seedTc 时写进粒子。
var qsr = __P$('q-range'), qv = __P$('q-val');
var curQ = function(){ var v = parseFloat(qsr && qsr.value); return isFinite(v) ? v : 1; };
var syncQVal = function(){
  var q = curQ();
  if (qv) qv.textContent = (q > 0 ? '+' : '') + q + ' μC';
  var bp = __P$('q-pos-btn'), bn = __P$('q-neg-btn'), bz = __P$('q-zero-btn');
  if (bp) bp.style.outline = (q > 0) ? '2px solid #00695C' : 'none';
  if (bn) bn.style.outline = (q < 0) ? '2px solid #6A1B9A' : 'none';
  if (bz) bz.style.outline = (q === 0) ? '2px solid #455A64' : 'none';
};

// 加载时就存在的粒子不打初速度（保持原样），只有后加的（下标 ≥ __initN）才落速
var __initN = (typeof testCharges !== 'undefined') ? testCharges.length : 0;
var seedTc = function(tc, fx, fy){
  if (!tc) return;
  tc.q = curQ();                         // 放入瞬间套用面板上先选好的正负/大小
  syncQVal();
  var sp = curV(), a = curT() * Math.PI / 180;
  // 原脚本的 add-test 用的是 Math.random() 随机坐标，老师没法指定位置 → 这里改成显式坐标
  // 注意：按钮路径调 seedTc(tc) 时不传坐标，必须回落到「放入位置」滑块，否则又变回随机
  if (fx === undefined || fx === null || !isFinite(fx)) fx = curX();
  if (fy === undefined || fy === null || !isFinite(fy)) fy = curY();
  if (isFinite(fx)) tc.x = fx;
  if (isFinite(fy)) tc.y = fy;
  tc.path = [];
  tc.v0 = sp; tc.th = curT();
  tc.vx = sp * Math.cos(a); tc.vy = sp * Math.sin(a);
  tc.__seen = true;                       // 打标记：只落一次速，后续不再被全局滑块覆盖
  var am = __P$('auto-motion');
  if (am && am.checked){ var sm = __P$('sim-motion'); if (sm) sm.checked = true; }
};
// 关键时序：补丁的监听器在「脚本解析时」注册，而原脚本的 .onclick 是在 window.onload→setupEvents()
// 里才赋值的 → 点击时本监听器 【先跑】，此刻新粒子还没 push 进 testCharges。
// 所以这里只做「延时兜底」；真正的落速放在 renderTestControls 包装里（那是在原 onclick 内部
// push 之后同步调用的，一定能拿到刚插入的粒子）。
var addBtn = __P$('add-test');
// 关键：原 add-test 的 onclick 里是「先 push（q 硬编码为 1、坐标随机），再同步 renderTestControls()」，
// 那些「push 之后才补 q / 速度」的补丁写法，卡片上显示的永远是旧值（q=1、随机坐标）。
// 所以这里改为自己构造粒子：面板上的 q / 位置 / v₀ / θ 一次性写全，再渲染卡片。
var pushOne = function(x, y){
  var tc = { x: x, y: y, q: curQ(), vx: 0, vy: 0, path: [], id: Date.now(), __seen: true };
  var sp = curV(), a = curT() * Math.PI / 180;
  tc.v0 = sp; tc.th = curT(); tc.vx = sp * Math.cos(a); tc.vy = sp * Math.sin(a);
  var am = __P$('auto-motion');
  if (am && am.checked){ var sm = __P$('sim-motion'); if (sm) sm.checked = true; }
  testCharges.push(tc);
  syncQVal();
  if (window.renderTestControls) window.renderTestControls();
  return tc;
};
if (addBtn) addBtn.addEventListener('click', function(){
  // 原 onclick 已 push 了一个「坐标随机」的粒子，这里把它挪到滑块指定的位置
  var tc = testCharges[testCharges.length - 1];
  if (tc && !tc.__seen){ tc.q = curQ(); seedTc(tc); }   // 面板上选的正负一并套进去
  setTimeout(function(){
    for (var i = __initN; i < testCharges.length; i++) if (!testCharges[i].__seen) seedTc(testCharges[i]);
  }, 0);
});

/* ---------- 二·五、试探电荷的「放置位置」 ---------- */
var pxr = __P$('px-range'), pyr = __P$('py-range'), pmBox = __P$('place-mode');
var curX = function(){ var v = parseFloat(pxr && pxr.value); return isFinite(v) ? v : (width / 2); };
var curY = function(){ var v = parseFloat(pyr && pyr.value); return isFinite(v) ? v : (height / 2); };
var placeOn = function(){ return !!(pmBox && pmBox.checked); };
var syncPosUI = function(x, y){
  if (pxr){ var mx = parseFloat(pxr.max); if (isFinite(mx)) x = Math.max(20, Math.min(mx, Math.round(x))); pxr.value = x; }
  if (pyr){ var my = parseFloat(pyr.max); if (isFinite(my)) y = Math.max(20, Math.min(my, Math.round(y))); pyr.value = y; }
  var lx = __P$('px-val'), ly = __P$('py-val');
  if (lx) lx.textContent = pxr ? pxr.value : '';
  if (ly) ly.textContent = pyr ? pyr.value : '';
};
// 复用原按钮逻辑（保证 q/属性一致），再把粒子挪到指定坐标
var addOne = function(x, y){ return pushOne(x, y); };
var __cursor = null;      // 鼠标/触摸在画布上的位置，用于画"待放置"预览
var __downHit = false;    // 刚才那一下是否按在某个电荷上（是的话是拖拽，不要放粒子）
var cvs = document.getElementById('fieldCanvas');
if (cvs){
  var cvsPos = function(ce){ var r = cvs.getBoundingClientRect(); return { x: (ce.clientX || (ce.touches && ce.touches[0] && ce.touches[0].clientX)) - r.left, y: (ce.clientY || (ce.touches && ce.touches[0] && ce.touches[0].clientY)) - r.top }; };
  cvs.addEventListener('mousemove', function(e){ var p = cvsPos(e); __cursor = p; });
  cvs.addEventListener('mouseleave', function(){ __cursor = null; });
  cvs.addEventListener('mousedown', function(e){
    var p = cvsPos(e); __downHit = false;
    for (var i = 0; i < sourceCharges.length; i++)
      if (Math.hypot(sourceCharges[i].x - p.x, sourceCharges[i].y - p.y) < 30) __downHit = true;
  });
  var tryPlace = function(x, y){
    if (!placeOn()) return;
    if (x < 0 || y < 0 || x > width || y > height) return;
    if (__downHit) return;                                  // 在拖场源电荷，别顺手放粒子
    var tc = addOne(x, y);
    if (tc){ syncPosUI(x, y); window.renderTestControls(); }
  };
  cvs.addEventListener('click', function(e){
    var p = cvsPos(e); tryPlace(p.x, p.y);
  });
  cvs.addEventListener('touchstart', function(e){
    var p = cvsPos(e); __cursor = p;
  }, { passive: true });
  cvs.addEventListener('touchend', function(e){
    var t = e.changedTouches && e.changedTouches[0]; if (!t) return;
    var r = cvs.getBoundingClientRect();
    e.preventDefault();                                     // 阻止合成 click，避免触摸放两个
    tryPlace(t.clientX - r.left, t.clientY - r.top);
  }, { passive: false });
}
// 「待放置位置」预览：十字 + 圆圈 + 速度方向箭头
var drawPlaceCursor = function(){
  if (!placeOn() || !__cursor) return;
  var c = __cursor;
  ctx.save();
  ctx.strokeStyle = 'rgba(0,137,123,0.8)'; ctx.lineWidth = 1.2;
  ctx.setLineDash([4, 4]);
  ctx.beginPath();
  ctx.moveTo(c.x - 16, c.y); ctx.lineTo(c.x - 7, c.y);
  ctx.moveTo(c.x + 7, c.y);  ctx.lineTo(c.x + 16, c.y);
  ctx.moveTo(c.x, c.y - 16); ctx.lineTo(c.x, c.y - 7);
  ctx.moveTo(c.x, c.y + 7);  ctx.lineTo(c.x, c.y + 16);
  ctx.stroke();
  ctx.setLineDash([]);
  var qc = curQ();
  var qcol = (qc > 0) ? '#4CAF50' : ((qc < 0) ? '#9C27B0' : '#90A4AE');
  ctx.strokeStyle = qcol; ctx.lineWidth = 2.2;
  ctx.beginPath(); ctx.arc(c.x, c.y, 9, 0, 6.2832); ctx.stroke();
  ctx.fillStyle = qcol; ctx.font = 'bold 12px Arial'; ctx.textAlign = 'center';
  ctx.fillText(qc > 0 ? '+' : (qc < 0 ? '-' : '0'), c.x, c.y + 4);
  ctx.lineWidth = 1.2;
  var sp = curV(), a = curT() * Math.PI / 180, L = Math.min(sp * 7, 95);
  if (sp > 0.05){
    ctx.strokeStyle = '#00897B'; ctx.lineWidth = 2;
    ctx.beginPath(); ctx.moveTo(c.x, c.y); ctx.lineTo(c.x + L * Math.cos(a), c.y + L * Math.sin(a)); ctx.stroke();
    ctx.fillStyle = '#00897B';
    ctx.beginPath();
    ctx.moveTo(c.x + L * Math.cos(a), c.y + L * Math.sin(a));
    ctx.lineTo(c.x + L * Math.cos(a) - 9 * Math.cos(a) - 4.5 * Math.sin(a), c.y + L * Math.sin(a) - 9 * Math.sin(a) + 4.5 * Math.cos(a));
    ctx.lineTo(c.x + L * Math.cos(a) - 9 * Math.cos(a) + 4.5 * Math.sin(a), c.y + L * Math.sin(a) - 9 * Math.sin(a) - 4.5 * Math.cos(a));
    ctx.closePath(); ctx.fill();
  }
  ctx.restore();
};

var origRender = window.renderTestControls;
window.renderTestControls = function(){
  // 原 onclick 是「先 push（q 硬编码 1、坐标随机）→ 再同步 renderTestControls()」，
  // 所以凡是「push 之后才补 q」的写法，卡片上显示的永远是 q=1。
  // 这里在 origRender 之前补写，卡片取到的就是面板上刚选好的正负。
  for (var k = __initN; k < testCharges.length; k++){
    if (testCharges[k] && !testCharges[k].__seen) testCharges[k].q = curQ();
  }
  if (origRender) origRender();
  // 位置滑块的量程跟画布实际尺寸对齐
  if (pxr && width) { pxr.max = Math.max(60, Math.round(width - 20)); if (pxr.value > width - 20) pxr.value = Math.round(width - 20); }
  if (pyr && height){ pyr.max = Math.max(60, Math.round(height - 20)); if (pyr.value > height - 20) pyr.value = Math.round(height - 20); }
  var box = __P$('test-list');
  if (!box) return;
  // 原 onclick 里 push(tc) 之后紧接着就调 renderTestControls → 这里能 100% 拿到新粒子
  for (var k = __initN; k < testCharges.length; k++) if (!testCharges[k].__seen) seedTc(testCharges[k]);
  testCharges.forEach(function(tc, idx){
    var item = box.children[idx];
    if (!item) return;
    if (tc.v0 === undefined){
      tc.v0 = Math.hypot(tc.vx || 0, tc.vy || 0);
      tc.th = ((Math.atan2(tc.vy || 0, tc.vx || 0) * 180 / Math.PI) + 360) % 360;
    }
    if (item.querySelector('.pv')) return;   // 已经加过，避免重复叠
    var row = document.createElement('div');
    row.style.cssText = 'margin-top:7px; border-top:1px dashed #ddd; padding-top:7px;';
    row.innerHTML =
      '<div class="label-row"><span style="font-size:0.78rem;">v₀ 大小</span><span style="font-size:0.78rem;">' + (tc.v0 || 0).toFixed(1) + '</span></div>' +
      '<input type="range" min="0" max="15" step="0.5" value="' + (tc.v0 || 0) + '" class="pv">' +
      '<div class="label-row"><span style="font-size:0.78rem;">方向 θ</span><span style="font-size:0.78rem;">' + (tc.th || 0) + '°</span></div>' +
      '<input type="range" min="0" max="360" step="5" value="' + (tc.th || 0) + '" class="pt">';
    item.appendChild(row);
    var sl = row.querySelector('.pv'), sr = row.querySelector('.pt');
    var labV = row.children[0] && row.children[0].children[1];   // v₀ 数值
    var labT = row.children[2] && row.children[2].children[1];   // θ 数值
    function apply(sp, dg){
      tc.v0 = sp; tc.th = dg;
      var a = dg * Math.PI / 180;
      tc.vx = sp * Math.cos(a); tc.vy = sp * Math.sin(a);
      if (labV) labV.textContent = sp.toFixed(1);
      if (labT) labT.textContent = dg + '°';
    }
    if (sl) sl.oninput = function(){ apply(parseFloat(sl.value) || 0, tc.th); };
    if (sr) sr.oninput = function(){ apply(tc.v0 || 0, parseFloat(sr.value) || 0); };
  });
};

var origDraw = window.drawTestCharge;
window.drawTestCharge = function(tc){
  if (origDraw) origDraw(tc);
  var sp = Math.hypot(tc.vx || 0, tc.vy || 0);
  if (sp < 0.05) return;
  var L = Math.min(sp * 7, 95), a = Math.atan2(tc.vy, tc.vx);
  var ex = tc.x + L * Math.cos(a), ey = tc.y + L * Math.sin(a);
  ctx.save();
  ctx.strokeStyle = '#00897B'; ctx.lineWidth = 2;
  ctx.beginPath(); ctx.moveTo(tc.x, tc.y); ctx.lineTo(ex, ey); ctx.stroke();
  ctx.translate(ex, ey); ctx.rotate(a);
  ctx.fillStyle = '#00897B';
  ctx.beginPath(); ctx.moveTo(0, 0); ctx.lineTo(-9, -4.5); ctx.lineTo(-9, 4.5); ctx.closePath(); ctx.fill();
  ctx.restore();
  ctx.save();
  ctx.fillStyle = '#00695C'; ctx.font = 'bold 11px Arial'; ctx.textAlign = 'left';
  ctx.fillText('v=' + sp.toFixed(1), tc.x + 15, tc.y + 25);
  ctx.restore();
};

var origAnimate = window.animate;
window.animate = function(){
  if (origAnimate) origAnimate();
  try { drawEquipotentials(); } catch (e) { if (window.console && console.warn) console.warn('[155补丁] 等势面', e); }
  try { drawPlaceCursor(); } catch (e) { if (window.console && console.warn) console.warn('[155补丁] 放置预览', e); }
};

/* ---------- 三、初始化与绑定（放最后，逐项隔离错误） ---------- */
__safe(function(){ var b1 = __P$('q-pos-btn'), b2 = __P$('q-neg-btn'), b3 = __P$('q-zero-btn');
  if (qsr) qsr.oninput = syncQVal;
  if (b1) b1.onclick = function(){ qsr.value = 1;  syncQVal(); };
  if (b2) b2.onclick = function(){ qsr.value = -1; syncQVal(); };
  if (b3) b3.onclick = function(){ qsr.value = 0;  syncQVal(); };
  syncQVal();
});
__safe(function(){ if (v0r) v0r.oninput = function(){ var s = curV(); var lab = __P$('v0-val'); if (lab) lab.textContent = s.toFixed(1); }; });
__safe(function(){ if (thr) thr.oninput = function(){ var s = curT(); var lab = __P$('th-val'); if (lab) lab.textContent = s + '°'; }; });
__safe(function(){ if (eqBtn) eqBtn.onclick = function(){ toggleEq(); }; });
__safe(function(){ if (eqBox) eqBox.onchange = function(){ toggleEq(); }; });
__safe(function(){ if (eqSteps) eqSteps.oninput = syncEqVal; });
__safe(syncEqVal);
__safe(function(){ toggleEq(false); });
__safe(function(){
  if (pxr) pxr.oninput = function(){ syncPosUI(parseFloat(pxr.value) || 0, curY()); };
  if (pyr) pyr.oninput = function(){ syncPosUI(curX(), parseFloat(pyr.value) || 0); };
});
// 初始位置默认放在画布正中，别再让老师靠"碰运气"
__safe(function(){
  if (pxr && pyr && !pxr.value){ pxr.value = Math.round(width / 2); }
  if (pyr && !pyr.value){ pyr.value = Math.round(height / 2); }
  syncPosUI(curX(), curY());
  if (addBtn) addBtn.textContent = '+ 放入试探电荷（指定位置/点画布）';
});
