'use strict';
(async()=>{
  const el=id=>document.getElementById(id);
  const labels={format:'Rust formatting',clippy:'Strict Clippy',rust:'Rust unit & integration',protocols:'Protocol & recovery contracts','report-exporter':'Report integrity',gateway:'Public gateway isolation',community:'Private node submissions','web-build':'Website fixture build',web:'Browser workflows'};
  try{
    const response=await fetch('/testing/report.json',{cache:'no-cache'});if(!response.ok)throw Error('Results unavailable');const data=await response.json();
    if(!data.reports?.length){el('verification-status').textContent='A verification snapshot has not been published yet.';return;}
    const groups=data.reports.flatMap(r=>r.groups.map(g=>({...g,origin:r.origin,platform:r.platform}))),sum=key=>groups.reduce((total,g)=>total+g[key],0);
    const passed=data.status==='passed',local=data.reports.every(r=>r.origin==='local');el('verification-status').textContent=passed?(local?'Local verification passed.':'Recorded checks passed.'):'Recorded verification includes failures. Review the results before using this revision.';
    el('verification-status').className='verification-state '+(passed?'passed':'failed');
    for(const key of ['passed','failed','skipped'])el('verification-'+key).textContent=sum(key).toLocaleString();
    const source=el('verification-source');source.textContent='Tested '+data.source_commit.slice(0,12)+' · recorded '+new Date(data.recorded_at).toLocaleString()+'. ';
    const link=document.createElement('a');link.href='https://github.com/RIRRd/RIRRd/commit/'+data.source_commit;link.textContent='Exact tested revision ↗';source.append(link);
    const fragment=document.createDocumentFragment();
    for(const g of groups){const row=document.createElement('tr');for(const [i,value]of [labels[g.id]||g.id,g.status,g.passed,g.failed,g.skipped].entries()){const cell=document.createElement(i?'td':'th');cell.textContent=String(value);if(!i){cell.scope='row';const note=document.createElement('small');note.textContent=g.origin.replaceAll('_',' ')+' · '+g.platform;cell.append(note);}row.append(cell);}fragment.append(row);}el('verification-groups').replaceChildren(fragment);
    RIRRdCharts.bars(el('verification-chart'),groups.filter(g=>g.tests>0).map(g=>({label:labels[g.id]||g.id,value:g.passed})),'Passed cases by area');
    if(data.coverage){const lines=data.coverage.lines;el('coverage-description').textContent=lines.percent.toFixed(2)+'% of measured source lines executed ('+lines.covered.toLocaleString()+' / '+lines.count.toLocaleString()+').';el('coverage-meter').value=lines.percent;el('coverage-meter').hidden=false;}
    if(data.ci_status==='verified_workflow' && data.verified_ci?.source_commit===data.source_commit && data.verified_ci?.conclusion==='success' && /^https:\/\/github\.com\/RIRRd\/RIRRd\/actions\/runs\/[0-9]+$/.test(data.verified_ci.url)){
      const ci=data.verified_ci,note=el('ci-verification-note');note.textContent='The linked GitHub workflow passed at this tested revision, including Linux x86-64, ARM64, Windows, browser tests, coverage and dependency auditing. The table above retains its recorded case-count origin. ';
      const link=document.createElement('a');link.href=ci.url;link.textContent='Verified '+ci.workflow+' run ↗';note.append(link);
      const details=document.createElement('details'),summary=document.createElement('summary'),list=document.createElement('ul');summary.textContent=ci.jobs.length+' successful CI jobs';details.append(summary,list);
      for(const job of ci.jobs){const item=document.createElement('li');item.textContent=job.name+' — passed';list.append(item);}note.insertAdjacentElement('afterend',details);
    }else if(data.ci_status==='recorded_run'){el('ci-verification-note').textContent='Core and browser results were exported from the linked GitHub Actions runs. Open CI to inspect job logs and JUnit/coverage artifacts.';for(const url of new Set(data.reports.map(r=>r.run_url))){if(!/^https:\/\/github\.com\/[^/]+\/[^/]+\/actions\/runs\/[0-9]+$/.test(url))continue;const link=document.createElement('a');link.href=url;link.textContent=' Workflow run '+url.split('/').pop()+' ↗';el('ci-verification-note').append(link);}}
  }catch{el('verification-status').textContent='Recorded test results are temporarily unavailable. The CI workflow and reproduction commands remain available.';}
})();
