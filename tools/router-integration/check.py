#!/usr/bin/env python3
"""Exercise actual bgpq4 output, guarded generation and isolated router parsers."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--candidate',required=True)
parser.add_argument('--reference',help='pinned official IRRd, loaded with the same EXAMPLE fixture')
parser.add_argument('--router-image',default='rirrd-router-tools:local')
args=parser.parse_args()
root=Path(__file__).resolve().parents[2]
expected4={'192.0.2.0/24','198.51.100.0/24'}
expected6={'2001:db8:100::/48','2001:db8:200::/48'}
cases=[]
for family in ['4','6']:
    for format_args in [['-j'],['-b'],[]]:
        cases.append((family,format_args,['AS64496:AS-DOCS']))
    cases.append((family,['-j'],['RS-DOCS']))
    cases.append((family,['-j'],['AS64496:AS-DOCS','EXCEPT','AS64497']))
cases += [('4',['-b','-t'],['AS64496:AS-DOCS']),('4',['-f','42'],['AS64496:AS-DOCS'])]
for family,flags,objects in cases:
    outputs=[]
    for endpoint in [args.candidate]+([args.reference] if args.reference else []):
        result=subprocess.run(['bgpq4','-p','-'+family,'-h',endpoint,'-S','EXAMPLE','-l','DOCS']+flags+objects,capture_output=True,text=True,timeout=15)
        assert result.returncode==0 and not result.stderr,(flags,objects,result.stderr)
        if flags==['-j']:
            prefixes={entry['prefix'] for entry in json.loads(result.stdout)['DOCS']}
            expected=expected4 if family=='4' else expected6
            if 'EXCEPT' in objects:expected=expected-({'198.51.100.0/24'} if family=='4' else {'2001:db8:200::/48'})
            assert prefixes==expected,(objects,prefixes,expected)
        outputs.append(result.stdout)
    assert all(output==outputs[0] for output in outputs),(family,flags,objects,'oracle differs')
print(f'{len(cases)} real bgpq4 profiles passed'+(' and matched pinned upstream' if args.reference else ''))
with tempfile.TemporaryDirectory(prefix='rirrd-router-check-') as state:
    state=Path(state)
    output=state/'candidate'
    base=['python3',str(root/'tools/router-integration/generate_filters.py'),'--endpoint',args.candidate,'--sources','EXAMPLE','--object','AS64496:AS-DOCS','--name','DOCS_IRR','--allow-reserved-asns']
    subprocess.run(base+['--output',str(output)],check=True,capture_output=True)
    original={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    for suffix in [[],['--sources','UNKNOWN'],['--object','AS-MISSING'],['--max-prefixes','1'],['--endpoint','127.0.0.1:1']]:
        destination=output if not suffix else state/('failed-'+str(len(suffix))+'-'+suffix[-1].replace(':','_'))
        failed=subprocess.run(base+suffix+['--output',str(destination)],capture_output=True,timeout=40)
        assert failed.returncode!=0
        if destination!=output:assert not destination.exists(),'failed generation published a policy'
        assert original=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()},'last good bundle changed'
    bird=(root/'examples/routers/bird.conf').read_text().replace('/etc/bird/rirrd/filters.bird','/work/filters.bird')
    (output/'bird.conf').write_text(bird)
    (output/'frr.conf').write_text((output/'filters.frr').read_text()+(root/'examples/routers/frr.conf').read_text())
    # These are synthetic, read-only parser fixtures. Container root has no
    # DAC override capability and is not the GitHub runner's host UID. Keep the
    # generator's private production defaults; make only this test bundle readable.
    output.chmod(0o755)
    for file in output.iterdir():
        file.chmod(0o644)
    docker=['docker','run','--rm','--network','none','--cap-drop','ALL','--security-opt','no-new-privileges','--ulimit','nofile=1024:1024','--memory','256m','--cpus','1','-v',str(output)+':/work:ro',args.router_image]
    for command in [['bird','-p','-u','root','-g','root','-c','/work/bird.conf'],['/usr/lib/frr/bgpd','-C','-S','--limit-fds','1024','-f','/work/frr.conf']]:
        result=subprocess.run(docker+command,capture_output=True,text=True,timeout=30)
        assert result.returncode==0 and not result.stderr,(command,result.stdout,result.stderr)
    (output/'bird.conf').write_text(bird+'\nTHIS_IS_NOT_VALID_CONFIGURATION;\n')
    result=subprocess.run(docker+['bird','-p','-u','root','-g','root','-c','/work/bird.conf'],capture_output=True,timeout=30)
    assert result.returncode!=0,'BIRD accepted invalid candidate'
print('BIRD and FRR config parsers passed under 1 CPU / 256 MiB, network-none; invalid BIRD config denied')
print('Five generator failures preserved the last good bundle and published no failed candidate')
