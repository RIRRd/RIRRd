'use strict';
// Local SVG charts. Reuse unchanged drawings and preserve the inspected sample.
window.RIRRdCharts = (() => {
  const ns = 'http://www.w3.org/2000/svg', drawings = new WeakMap();
  function node(tag, attrs = {}, label) { const e=document.createElementNS(ns,tag);for(const [k,v] of Object.entries(attrs))e.setAttribute(k,String(v));if(label!=null)e.textContent=label;return e; }
  const width = container => Math.max(240,Math.min(680,Math.round(container.clientWidth || 680)));
  function prepare(container, signature, render) {
    let state=drawings.get(container);
    if(!state){state={signature:null,followLatest:true,selected:null};drawings.set(container,state);if(window.ResizeObserver){state.observer=new ResizeObserver(entries=>{if(entries[0].contentRect.width>0)state.render?.();});state.observer.observe(container);}}
    state.render=render;
    if(state.signature===signature && container.firstChild)return null;
    state.focused=container.contains(document.activeElement);state.signature=signature;return state;
  }
  function base(container,title,w,height=220){container.replaceChildren();const svg=node('svg',{viewBox:`0 0 ${w} ${height}`,role:'img','aria-label':title});svg.append(node('title',{},title));container.append(svg);return svg;}
  function empty(container,message){if(!prepare(container,'empty:'+message,()=>empty(container,message)))return;container.replaceChildren();const p=document.createElement('p');p.className='chart-empty';p.textContent=message;container.append(p);container.dataset.points='0';}
  function line(container,points,key,title,format=String,{events=[]}={}){
    const valid=points.filter(p=>Number.isFinite(p[key])&&Number.isFinite(p.timestamp));
    if(!valid.length)return empty(container,'Waiting for observations. No history is generated to fill gaps.');
    const w=width(container),signature=JSON.stringify([w,key,title,points.map(p=>[p.timestamp,p[key],p.run_id,p.run_kind]),events]);
    const state=prepare(container,signature,()=>line(container,points,key,title,format,{events}));if(!state)return;
    const svg=base(container,title+'; '+valid.length+' observations; latest '+format(valid.at(-1)[key]),w);container.dataset.points=String(valid.length);
    const left=76,right=w-12,top=15,bottom=175;
    let first=points[0].timestamp,last=points.at(-1).timestamp;if(first===last){first-=30;last+=30;}
    const max=Math.max(...valid.map(p=>p[key]),0)*1.12||1;
    const x=t=>left+(t-first)/(last-first)*(right-left),y=v=>bottom-v/max*(bottom-top);
    for(let i=0;i<4;i++){const v=max*i/3;svg.append(node('line',{x1:left,x2:right,y1:y(v),y2:y(v),class:'chart-grid'}),node('text',{x:left-9,y:y(v)+4,'text-anchor':'end',class:'chart-label'},format(v)));}
    for(const t of [first,last])svg.append(node('text',{x:x(t),y:202,'text-anchor':t===first?'start':'end',class:'chart-label'},new Date(t*1000).toLocaleTimeString([],{hour:'2-digit',minute:'2-digit'})));
    for(const event of events){const from=Math.max(first,event.start),to=Math.min(last,event.end);if(from>=to)continue;const band=node('rect',{x:x(from),y:top,width:x(to)-x(from),height:bottom-top,class:'chart-maintenance'});band.append(node('title',{},event.label));svg.append(band);}
    let group=[],previous,previousRun;const segments=[];
    for(const p of points){if(!Number.isFinite(p[key])||(previous&&(p.timestamp-previous>150||p.run_id!==previousRun))){if(group.length)segments.push(group);group=[];}if(Number.isFinite(p[key]))group.push(p);previous=p.timestamp;previousRun=p.run_id;}if(group.length)segments.push(group);
    for(const part of segments){svg.append(node('polyline',{points:part.map(p=>`${x(p.timestamp)},${y(p[key])}`).join(' '),class:'chart-line',fill:'none'}));if(part.length===1)svg.append(node('circle',{cx:x(part[0].timestamp),cy:y(part[0][key]),r:3,class:'chart-dot'}));}
    const marker=node('circle',{r:4,class:'chart-dot',visibility:'hidden'});svg.append(marker);
    const hint=document.createElement('p');hint.className='chart-readout';container.append(hint);
    const controls=document.createElement('input');controls.type='range';controls.min='0';controls.max=String(valid.length-1);controls.setAttribute('aria-label',title+': inspect sample');controls.className='chart-slider';
    const latest=document.createElement('button');latest.type='button';latest.className='chart-latest';latest.textContent='Latest';latest.setAttribute('aria-label',title+': follow latest sample');
    const actions=document.createElement('div');actions.className='chart-controls';actions.append(controls,latest);container.append(actions);
    function select(index,change=true){const p=valid[index];if(change){state.selected=p.timestamp;state.followLatest=index===valid.length-1;}controls.value=String(index);marker.setAttribute('cx',x(p.timestamp));marker.setAttribute('cy',y(p[key]));marker.setAttribute('visibility',state.followLatest?'hidden':'visible');hint.textContent=(state.followLatest?'Latest: ':'Selected: ')+format(p[key])+' · '+new Date(p.timestamp*1000).toLocaleString()+(p.run_kind==='maintenance'?' · Maintenance run':'');controls.setAttribute('aria-valuetext',hint.textContent);latest.disabled=state.followLatest;}
    function nearest(time){let lo=0,hi=valid.length-1;while(lo<hi){const mid=(lo+hi)>>1;if(valid[mid].timestamp<time)lo=mid+1;else hi=mid;}return lo>0&&Math.abs(valid[lo-1].timestamp-time)<Math.abs(valid[lo].timestamp-time)?lo-1:lo;}
    svg.addEventListener('pointermove',event=>{const r=svg.getBoundingClientRect(),position=(event.clientX-r.left)/r.width*w;select(nearest(first+(position-left)/(right-left)*(last-first)));});
    controls.addEventListener('input',()=>select(Number(controls.value)));latest.addEventListener('click',()=>select(valid.length-1));
    select(state.followLatest||state.selected==null?valid.length-1:nearest(state.selected),false);
    if(state.focused)controls.focus({preventScroll:true});
  }
  function bars(container,values,title,format=n=>Math.round(n).toLocaleString()){
    const rows=values.filter(v=>Number.isFinite(v.value)&&v.value>=0);if(!rows.length)return empty(container,'No measured data available.');
    const w=width(container),state=prepare(container,JSON.stringify([w,title,rows.map(r=>[r.label,r.value,!!r.reference,format(r.value)])]),()=>bars(container,values,title,format));if(!state)return;
    const h=rows.length*32+18,svg=base(container,title+'; '+rows.map(r=>r.label+': '+format(r.value)).join('; '),w,h),max=Math.max(...rows.map(r=>r.value),1),left=w<420?100:145,right=w-80;container.dataset.points=String(rows.length);
    rows.forEach((r,i)=>{const y=12+i*32;svg.append(node('text',{x:0,y:y+14,class:'chart-label'},r.label));const rect=node('rect',{x:left,y,width:Math.max(0,r.value/max*(right-left)),height:20,rx:4,class:r.reference?'chart-bar reference':'chart-bar'});rect.append(node('title',{},r.label+': '+format(r.value)));svg.append(rect,node('text',{x:w-1,y:y+14,'text-anchor':'end',class:'chart-label'},format(r.value)));});
  }
  return {line,bars,empty};
})();
