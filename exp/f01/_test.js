console.log('--- resize() 自适应高度 ---');
[900, 1200, 500, 300].forEach(function(h){
  cv.parentElement.clientHeight=h;
  resize();
  console.log('舞台高='+h+' -> 画布 W='+W+' H='+H+' (style.height='+cv.style.height+', 属性='+cv.height+')');
});
cv.parentElement.clientHeight=900; resize();

console.log('--- 连续 resize 是否稳定（不反复伸缩）---');
var seq=[];
for(var i=0;i<4;i++){ resize(); seq.push(H); }
console.log('连续4次 resize 后 H: '+seq.join(' -> '));

console.log('--- 绘制冒烟 ---');
[[-10,10,0,0.1,1,0,0.3],[-10,-10,90,2,20,1,1.5],[10,10,45,1,8.2,0.5,0.4],[0,0,37,0.75,12,0.25,0.8]].forEach(function(a){
  vb=a[0]; v0=a[1]; th=a[2]*Math.PI/180; mu=a[3]; L=a[4]; e=a[5]; r=a[6];
  reset();
  for(var k=0;k<300 && !off;k++) advance(0.008*rate);
  draw();
  console.log('θ='+a[2]+' μ='+mu+' L='+L+' -> t='+t.toFixed(2)+' ok');
});
console.log('stroke='+calls.stroke+' fill='+calls.fill+' arc='+calls.arc);
console.log('SMOKE_OK');
