'use strict';
// Small local SVG charts. Every plotted point comes from the supplied data.
window.RIRRdCharts = (() => {
  const ns = 'http://www.w3.org/2000/svg';
  function node(tag, attrs = {}, label) { const e = document.createElementNS(ns, tag); for (const [k,v] of Object.entries(attrs)) e.setAttribute(k,String(v)); if (label != null) e.textContent = label; return e; }
  function base(container, title, height = 220) { container.replaceChildren(); const svg = node('svg',{viewBox:`0 0 680 ${height}`,role:'img','aria-label':title}); svg.append(node('title',{},title)); container.append(svg); return svg; }
  function empty(container, message) { container.replaceChildren(); const p=document.createElement('p');p.className='chart-empty';p.textContent=message;container.append(p);container.dataset.points='0'; }
  function line(container, points, key, title, format = String) {
    const valid=points.filter(p=>Number.isFinite(p[key]) && Number.isFinite(p.timestamp));
    if (!valid.length) return empty(container,'No samples yet. This chart starts with real observations; it has no generated history.');
    const svg=base(container,title); container.dataset.points=String(valid.length);
    const left=80,right=660,top=15,bottom=175;
    let first=Math.min(...points.map(p=>p.timestamp)),last=Math.max(...points.map(p=>p.timestamp));
    if(first===last){first-=30;last+=30;}
    const max=Math.max(...valid.map(p=>p[key]),0)*1.12 || 1;
    const x=t=>left+(t-first)/(last-first)*(right-left),y=v=>bottom-v/max*(bottom-top);
    for(let i=0;i<4;i++){const v=max*i/3;svg.append(node('line',{x1:left,x2:right,y1:y(v),y2:y(v),class:'chart-grid'}),node('text',{x:left-10,y:y(v)+4,'text-anchor':'end',class:'chart-label'},format(v)));}
    for(const t of [first,last]) svg.append(node('text',{x:x(t),y:202,'text-anchor':t===first?'start':'end',class:'chart-label'},new Date(t*1000).toLocaleTimeString([], {hour:'2-digit',minute:'2-digit'})));
    let segment=[]; const segments=[];let previous;
    for(const p of points){if(!Number.isFinite(p[key]) || (previous && p.timestamp-previous>150)){if(segment.length)segments.push(segment);segment=[];} if(Number.isFinite(p[key]))segment.push(p);previous=p.timestamp;}
    if(segment.length)segments.push(segment);
    for(const group of segments){svg.append(node('polyline',{points:group.map(p=>`${x(p.timestamp)},${y(p[key])}`).join(' '),class:'chart-line',fill:'none'}));if(group.length===1)svg.append(node('circle',{cx:x(group[0].timestamp),cy:y(group[0][key]),r:3,class:'chart-dot'}));}
    const marker=node('circle',{r:4,class:'chart-dot',visibility:'hidden'});svg.append(marker);
    const hint=document.createElement('p');hint.className='chart-readout';hint.textContent='Latest: '+format(valid.at(-1)[key])+' · '+new Date(valid.at(-1).timestamp*1000).toLocaleString();container.append(hint);
    function select(index){const p=valid[index];marker.setAttribute('cx',x(p.timestamp));marker.setAttribute('cy',y(p[key]));marker.setAttribute('visibility','visible');hint.textContent=format(p[key])+' · '+new Date(p.timestamp*1000).toLocaleString();}
    svg.addEventListener('pointermove',event=>{const rect=svg.getBoundingClientRect(),position=(event.clientX-rect.left)/rect.width*680;const time=first+(position-left)/(right-left)*(last-first);let nearest=0;for(let i=1;i<valid.length;i++)if(Math.abs(valid[i].timestamp-time)<Math.abs(valid[nearest].timestamp-time))nearest=i;select(nearest);});
    const controls=document.createElement('input');controls.type='range';controls.min='0';controls.max=String(valid.length-1);controls.value=controls.max;controls.setAttribute('aria-label',title+': inspect sample');controls.className='chart-slider';controls.addEventListener('input',()=>select(Number(controls.value)));container.append(controls);
  }
  function bars(container, values, title, format = n=>Math.round(n).toLocaleString()) {
    const rows=values.filter(v=>Number.isFinite(v.value)&&v.value>=0);
    if(!rows.length)return empty(container,'No measured data available.');
    const height=rows.length*33+20,svg=base(container,title,height),max=Math.max(...rows.map(r=>r.value),1);container.dataset.points=String(rows.length);
    rows.forEach((r,i)=>{const y=15+i*33;svg.append(node('text',{x:0,y:y+13,class:'chart-label'},r.label));const rect=node('rect',{x:150,y,width:Math.max(1,r.value/max*400),height:20,class:r.reference?'chart-bar reference':'chart-bar'});rect.append(node('title',{},r.label+': '+format(r.value)));svg.append(rect,node('text',{x:565,y:y+14,class:'chart-label'},format(r.value)));});
  }
  return {line,bars,empty};
})();
