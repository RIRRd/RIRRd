#!/usr/bin/env python3
"""Compare baseline/candidate ROV kernels on synthetic payloads, never a live DB."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tomllib

ROOT=Path(__file__).resolve().parents[2]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--baseline',default='v0.1.0-beta.1')
parser.add_argument('--work',type=Path,required=True)
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
work=args.work.resolve()
if work.is_relative_to(ROOT):parser.error('Keep generated benchmarks outside the repository')
work.mkdir(parents=True,exist_ok=True)
baseline=subprocess.check_output(['git','rev-parse',args.baseline+'^{commit}'],cwd=ROOT,text=True).strip()
(work/'baseline.rs').write_bytes(subprocess.check_output(['git','show',baseline+':src/rpki.rs'],cwd=ROOT))
shutil.copyfile(ROOT/'src/rpki.rs',work/'candidate.rs')
lock=tomllib.loads((ROOT/'Cargo.lock').read_text());versions={p['name']:p['version'] for p in lock['package'] if p['name'] in ['anyhow','serde','serde_json','libc']}
(work/'Cargo.toml').write_text('[package]\nname="rirrd-rov-kernel-comparison"\nversion="0.0.0"\nedition="2024"\n[workspace]\n[dependencies]\nanyhow="='+versions['anyhow']+'"\nserde={version="='+versions['serde']+'",features=["derive"]}\nserde_json="='+versions['serde_json']+'"\n[[bin]]\nname="rov-compare"\npath="main.rs"\n')
with (work/'Cargo.toml').open('a') as manifest: manifest.write('\n[dependencies.libc]\nversion="='+versions['libc']+'"\n')
(work/'main.rs').write_text(r'''
#[allow(dead_code)] mod baseline;
#[allow(dead_code)] mod candidate;
use std::{hint::black_box,net::{IpAddr,Ipv4Addr,Ipv6Addr},time::Instant};
fn cpu_time()->f64{let mut t=std::mem::MaybeUninit::<libc::timespec>::uninit();assert_eq!(unsafe{libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID,t.as_mut_ptr())},0);let t=unsafe{t.assume_init()};t.tv_sec as f64+t.tv_nsec as f64/1e9}
fn main(){
 let mut payloads=Vec::with_capacity(1048576);
 for n in 0..65536u32 {for origin in 64496..64504 {payloads.push((IpAddr::V4(Ipv4Addr::from(0x0a000000|(n<<8))),24,origin,32));payloads.push((IpAddr::V6(Ipv6Addr::from((0x20010db8u128<<96)|(u128::from(n)<<80))),48,origin,128));}}
 for length in 8..=32 {payloads.push(("198.0.0.0".parse().unwrap(),length,64496,32));}
 for length in [16,18,19,20,22,24,26,28,29,30,32,33,34,35,36,37,38,39,40,42,44,45,46,47,48,49,50,51,52,54,56,57,60,64,96,128] {payloads.push(("2600::".parse().unwrap(),length,64496,128));}
 let build=Instant::now();let old=baseline::RoaTable::from_verified_prefixes(payloads.iter().copied()).unwrap();let old_build=build.elapsed().as_secs_f64();
 let build=Instant::now();let new=candidate::RoaTable::from_verified_prefixes(payloads.iter().copied()).unwrap();let new_build=build.elapsed().as_secs_f64();
 assert_eq!(old.len(),new.len());
 let mut rows=Vec::new();
 for mode in 0..9 {let queries:Vec<_>=(0..1024u32).map(|i|{let n=(i*61)%65536;match mode{
 0=>(format!("10.{}.{}.0/24",n>>8,n&255),64496),
 1=>(format!("10.{}.{}.1/32",n>>8,n&255),64496),
 2=>(format!("10.{}.{}.0/24",n>>8,n&255),65000),
 3=>(format!("172.16.{}.0/24",n&255),64496),
 4=>(format!("2001:db8:{n:x}::/48"),64496),
 5=>(format!("2001:db8:{n:x}::/64"),64496),
 6=>(format!("2001:db8:{n:x}::1/128"),64496),
 7=>(format!("2001:db8:{n:x}::/48"),65000),
 _=>(format!("2001:db9:{n:x}::/48"),64496)}}).collect();
 for (p,a) in &queries {assert_eq!(format!("{:?}",old.validate(p,*a).unwrap()),format!("{:?}",new.validate(p,*a).unwrap()));}
 for round in 0..3 {let mut timings=[0.0;2];let mut cpu=[0.0;2];let mut checks=[0usize;2];for side in if round%2==0{[0,1]}else{[1,0]}{for(p,a)in queries.iter().take(128){if side==0{black_box(old.validate(p,*a).unwrap());}else{black_box(new.validate(p,*a).unwrap());}}
 let cpu_start=cpu_time();let start=Instant::now();for i in 0..100000 {let(p,a)=&queries[i%queries.len()];checks[side]+=if side==0{match black_box(old.validate(p,*a).unwrap()){baseline::Validation::Valid=>1,baseline::Validation::Invalid=>2,baseline::Validation::NotFound=>3}}else{match black_box(new.validate(p,*a).unwrap()){candidate::Validation::Valid=>1,candidate::Validation::Invalid=>2,candidate::Validation::NotFound=>3}};}
 timings[side]=start.elapsed().as_secs_f64();cpu[side]=cpu_time()-cpu_start;}assert_eq!(checks[0],checks[1]);rows.push(serde_json::json!({"profile":(["v4_exact_valid","v4_more_specific_valid","v4_invalid","v4_not_found","v6_exact_valid","v6_more_specific_64","v6_more_specific_128","v6_invalid","v6_not_found"][mode]),"round":round,"queries":100000,"baseline_seconds":timings[0],"candidate_seconds":timings[1],"baseline_cpu_seconds":cpu[0],"candidate_cpu_seconds":cpu[1],"checksum":checks[0]}));}}
 println!("{}",serde_json::json!({"vrps":new.len(),"baseline_build_seconds":old_build,"candidate_build_seconds":new_build,"samples":rows}));
}
''')
subprocess.run(['cargo','build','--offline','--release','--manifest-path',str(work/'Cargo.toml')],check=True,stdout=subprocess.DEVNULL)
cpu=max(os.sched_getaffinity(0))
def affinity():os.sched_setaffinity(0,{cpu})
result=json.loads(subprocess.check_output([str(work/'target/release/rov-compare')],preexec_fn=affinity,text=True))
result.update({'kind':'single-thread synthetic ROV kernel; no HTTP/TCP/cryptographic validation or live database','baseline_commit':baseline,'candidate_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'driver_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'candidate_source_sha256':hashlib.sha256((work/'candidate.rs').read_bytes()).hexdigest(),'candidate_dirty':bool(subprocess.check_output(['git','status','--porcelain'],cwd=ROOT)),'cpu':cpu,'order':'three paired rounds, alternating baseline/candidate order'})
args.output.write_text(json.dumps(result,indent=2)+'\n')
for profile in sorted({r['profile'] for r in result['samples']}):
 rows=[r for r in result['samples'] if r['profile']==profile];ratio=sum(r['baseline_cpu_seconds'] for r in rows)/sum(r['candidate_cpu_seconds'] for r in rows);print(profile,round(ratio,2),'x CPU-normalized lookup rate')
