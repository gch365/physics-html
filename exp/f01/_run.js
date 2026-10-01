const calls={};
const ctxStub=new Proxy({},{get(t,k){
  if(k==='canvas') return {width:760,height:600};
  if(['fillStyle','strokeStyle','lineWidth','font','textAlign','textBaseline','lineCap','lineJoin'].includes(k)) return t[k];
  return function(){
    for(const x of arguments){ if(typeof x==='number' && !isFinite(x)) throw new Error('bad num ctx.'+k); }
    calls[k]=(calls[k]||0)+1;
  };
},set(t,k,v){ t[k]=v; return true; }});
const DEFAULTS={vb:'-5',v0:'0',th:'37',mu:'0.5',L:'8.2',e:'1',r:'0.4',rt:'0.4'};
const STEP={vb:'0.1',v0:'0.1',th:'1',mu:'0.01',L:'0.1',e:'0.01',r:'0.05',rt:'0.05'};
const MIN={vb:'-10',v0:'-10',th:'0',mu:'0.1',L:'1',e:'0',r:'0.3',rt:'0.1'};
const MAX={vb:'10',v0:'10',th:'90',mu:'2',L:'20',e:'1',r:'1.5',rt:'3'};
const els={};
const stage={
  clientHeight: 900, clientWidth: 900,
  children:[{getBoundingClientRect:()=>({height:28})}, {getBoundingClientRect:()=>({height:34})}],
  style:{}
};
function mk(id){
  const isCv = (id==='cv');
  const e={id, value:(DEFAULTS[id]||'0'), step:(STEP[id]||'1'), min:(MIN[id]||'0'), max:(MAX[id]||'1'),
    textContent:'', checked:true, className:'', dataset:{st:'0.5'},
    addEventListener(){}, closest(){return {dataset:{st:this.dataset.st}};},
    getContext(){return ctxStub;}, width:760, height:600, style:{}};
  if(isCv){
    e.parentElement=stage;
    e.getBoundingClientRect=()=>({width:900, height:parseFloat(e.style.height)||0});
  } else {
    e.getBoundingClientRect=()=>({width:760,height:600});
  }
  return e;
}
global.document={ getElementById:(id)=>(els[id]=els[id]||mk(id)), querySelectorAll:()=>[] };
global.window={devicePixelRatio:1, addEventListener(){}};
global.getComputedStyle=()=>({paddingTop:'12px', paddingBottom:'12px'});
global.requestAnimationFrame=()=>0;
const src=require('fs').readFileSync(__dirname+'/_sim.js','utf8');
const test=require('fs').readFileSync(__dirname+'/_test.js','utf8');
eval(src+'\n'+test);
