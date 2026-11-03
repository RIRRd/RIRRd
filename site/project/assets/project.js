'use strict';
(async()=>{
  const el=id=>document.getElementById(id),fmt=n=>Math.round(n).toLocaleString();
  const legacy={nodes:'https://irrd.tools/#nodes','submit-node':'https://irrd.tools/#submit-node',query:'https://irrd.tools/dashboard/#query',rpki:'https://irrd.tools/dashboard/#rpki',sources:'https://irrd.tools/dashboard/#sources'};
  if(legacy[location.hash.slice(1)]){location.replace(legacy[location.hash.slice(1)]);return;}
  function range(values,unit=''){const lo=Math.min(...values),hi=Math.max(...values);return (unit==='ms'?lo.toFixed(2):fmt(lo))+'–'+(unit==='ms'?hi.toFixed(2):fmt(hi))+(unit?' '+unit:'');}
  try{
    const response=await fetch('/comparison.json',{cache:'no-cache'});if(!response.ok)throw new Error();const data=await response.json();
    const median=a=>a.slice().sort((a,b)=>a-b)[Math.floor(a.length/2)];
    const bars=(target,values,title,format)=>RIRRdCharts.bars(el(target),[{label:'RIRRd',value:values[0]},{label:'IRRd stack',value:values[1],reference:true}],title,format);
    bars('import-chart',[median(data.imports.map(r=>r.candidate.seconds)),median(data.imports.map(r=>r.reference.seconds))],'Snapshot import seconds',n=>n.toFixed(1)+' s');
    bars('benchmark-memory',[data.idle_pss.candidate/1048576,data.idle_pss.reference/1048576],'Idle process PSS in MiB',n=>n.toFixed(1)+' MiB');
    bars('benchmark-disk',[data.disk.candidate_allocated_bytes/1048576,data.disk.reference_allocated_bytes/1048576],'Allocated database storage in MiB',n=>n.toFixed(1)+' MiB');
    const rows=data.profiles.filter(r=>r.phase==='workload'&&r.variant==='current');
    for(const mode of ['Whois','Graphql']){const a=rows.filter(r=>r.mode===mode&&r.system==='candidate'),b=rows.filter(r=>r.mode===mode&&r.system==='reference');const tr=document.createElement('tr');for(const [i,value]of [mode==='Whois'?'TCP WHOIS':'HTTP GraphQL',range(a.map(r=>r.queries_per_second)),range(b.map(r=>r.queries_per_second)),range(a.map(r=>r.p95_ms),'ms'),range(b.map(r=>r.p95_ms),'ms')].entries()){const cell=document.createElement(i?'td':'th');cell.textContent=value;if(!i)cell.scope='row';tr.append(cell);}el('benchmark-queries').append(tr);}
  }catch{for(const id of ['import-chart','benchmark-memory','benchmark-disk'])RIRRdCharts.empty(el(id),'Recorded benchmark data is unavailable.');}
  try{const response=await fetch('/dashboard/live.json',{cache:'no-store'});if(!response.ok)throw new Error();const data=await response.json();if(data.service!=='online')throw new Error();el('showcase-objects').textContent=fmt(data.total_objects);el('showcase-sources').textContent=String(data.sources.filter(s=>s.state==='loaded').length);el('showcase-observed').textContent='Live UK node counters · observed '+new Date(data.observed_at).toLocaleString()+'. These are not benchmark measurements.';}catch{el('showcase-observed').textContent='Live node counters are currently unavailable. Visit irrd.tools for node status.';}
})();
