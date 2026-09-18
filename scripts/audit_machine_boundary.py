#!/usr/bin/env python3
"""Fail if the three routed runtime members still import machine OS operations.
This is a capability-specific gate, NOT proof of whole-runtime OS isolation.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
from audit_dependencies import parse_nm

MEMBERS = {'gcenv.unix', 'PalUnix', 'cgroup'}
FORBIDDEN = {'sysconf', 'sysinfo', 'sched_getaffinity', 'sched_setaffinity',
             'sched_getcpu', 'getrlimit', 'getrlimit64',
             'minipal_get_cpu_max_possible_count', '__sched_cpualloc',
             '__sched_cpucount', '__sched_cpufree'}

def assess(rows):
    seen, connected, bypasses = set(), set(), []
    for owner, symbol, kind in rows:
        match = re.search(r'(gcenv\.unix|PalUnix|cgroup)\.cpp\.(?:o|obj)[\])]?$' , owner)
        if not match: continue
        member = match[1]
        seen.add(member)
        plain = symbol.split('@',1)[0]
        if kind == 'U' and plain == 'dotnet_pal_get_api': connected.add(member)
        if kind in ('U','w','v') and plain in FORBIDDEN:
            bypasses.append({'owner':owner,'symbol':symbol,'binding':kind})
    return {'scope':'machine capability in three runtime members only',
            'members':sorted(seen), 'missing_members':sorted(MEMBERS-seen),
            'unconnected_members':sorted(MEMBERS-connected), 'bypasses':bypasses,
            'machine_boundary_pass': seen == MEMBERS and connected == MEMBERS and not bypasses}

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('runtime',type=Path); p.add_argument('--output',type=Path,required=True)
    p.add_argument('--nm',default='nm'); a=p.parse_args()
    process = subprocess.run([a.nm,'-A','-P','-g',str(a.runtime)],check=True,capture_output=True,text=True)
    r=assess(parse_nm(process.stdout))
    r.update(sha256=hashlib.sha256(a.runtime.read_bytes()).hexdigest(),tool_warnings=process.stderr)
    a.output.parent.mkdir(parents=True,exist_ok=True)
    a.output.write_text(json.dumps(r,indent=2)+'\n')
    if not r['machine_boundary_pass']: raise SystemExit('machine boundary audit failed: '+json.dumps(r))
    print('MACHINE SOURCE IMPORT AUDIT PASS (not whole-runtime OS isolation)')
if __name__=='__main__': main()
