"""Disposable output-only Windows comparison for #997; never an acceptance gate."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path.cwd()
OUT = Path(os.environ['RUNNER_TEMP']) / 'native997-evidence'
OUT.mkdir(exist_ok=True)
INSTRUMENT = (Path(__file__).parent / 'instrument.py').read_text()
SOURCES = ['b138e7f636df62499da42197393c526d77a600d2', '604163990a49d4c3fb47553a114aaaa9ed825ff9']
FILES = ['crates/nessa-client-core/src/retained.rs', 'crates/nessa-server/tests/composition/read_only_online/fixtures/gateway.rs', 'crates/nessa-server/tests/composition/read_only_online/native_session.rs']
TEST = 'composition::read_only_online_tests::native_session::native_refusal_rechecks_pinned_status_without_purging_confirmed_partial_history'
report = {'acceptance': False, 'scope': 'Output-only synthetic focused Windows comparison; original assertions and five-second production budget unchanged', 'sources': [], 'complete': False}

def git(*args):
    return subprocess.check_output(['git', *args], cwd=ROOT, text=True).strip()

def run_test(executable, folder, name, delay=0, workers=0):
    env = dict(os.environ, NESSA_997_DELAY_FIRST_PAGE_MS=str(delay))
    loads = []
    started = time.monotonic()
    try:
        for _ in range(workers):
            loads.append(subprocess.Popen([sys.executable, '-c', 'import time\nend=time.monotonic()+50\nx=1\nwhile time.monotonic()<end: x=(x*1664525+1013904223)%4294967296'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        with (folder / (name + '.log')).open('w', encoding='utf-8') as output:
            child = subprocess.Popen([executable, '--exact', TEST, '--nocapture', '--test-threads=1'], cwd=ROOT, env=env, stdout=output, stderr=subprocess.STDOUT)
            try:
                code = child.wait(timeout=60)
            except subprocess.TimeoutExpired:
                subprocess.run(['taskkill', '/PID', str(child.pid), '/T', '/F'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                child.wait(timeout=10)
                code = 'timeout'
        text = (folder / (name + '.log')).read_text(encoding='utf-8', errors='replace')
        selected = [line for line in text.splitlines() if 'N997' in line or 'left:' in line or 'right:' in line or 'test result:' in line]
        return {'name': name, 'delay_ms': delay, 'cpu_workers': workers, 'exit': code, 'seconds': round(time.monotonic()-started, 3), 'observations': selected}
    finally:
        for load in loads:
            if load.poll() is None:
                load.terminate()
            load.wait(timeout=10)

try:
    for sha in SOURCES:
        git('switch', '--detach', sha)
        folder = OUT / sha[:12]
        folder.mkdir(exist_ok=True)
        source = {'sha': git('rev-parse', 'HEAD'), 'tree': git('rev-parse', 'HEAD^{tree}'), 'original_source_sha256': {name: hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in FILES}, 'tests': []}
        report['sources'].append(source)
        exec(compile(INSTRUMENT, 'private-instrumentation', 'exec'), {})
        source['instrumented_source_sha256'] = {name: hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in FILES}
        command = ['cargo', 'test', '--no-run', '--message-format=json']
        for package in ['nessa-local-storage','nessa-auth','nessa-server','nessa-protocol','nessa-client-core','nessa-device-client','nessa-sdk']:
            command += ['-p', package]
        with (folder/'build.jsonl').open('w',encoding='utf-8') as output, (folder/'build.log').open('w',encoding='utf-8') as errors:
            build = subprocess.run(command, cwd=ROOT, stdout=output, stderr=errors)
        if build.returncode:
            source['build_exit'] = build.returncode
            raise RuntimeError('Focused executable did not build for '+sha)
        executables = []
        for line in (folder/'build.jsonl').read_text(encoding='utf-8').splitlines():
            try:
                item=json.loads(line)
            except json.JSONDecodeError:
                continue
            if item.get('reason')=='compiler-artifact' and item.get('target',{}).get('name')=='nessa_server' and item.get('profile',{}).get('test') and item.get('executable'):
                executables.append(item['executable'])
        if len(executables)!=1:
            raise RuntimeError('Expected one real library executable: '+repr(executables))
        source['executable_sha256']=hashlib.sha256(Path(executables[0]).read_bytes()).hexdigest()
        cases=[('normal',0,0),('loaded-1',0,4),('loaded-2',0,4),('loaded-3',0,4),('delayed-first-page',6000,0)]
        for name,delay,workers in cases:
            result=run_test(executables[0],folder,name,delay,workers)
            source['tests'].append(result)
            print(json.dumps({'source':sha,**result}),flush=True)
            (OUT/'summary.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
        git('restore','--source=HEAD','--',*FILES)
        source['restored_source_sha256']={name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in FILES}
        assert source['restored_source_sha256']==source['original_source_sha256']
    report['complete']=True
finally:
    subprocess.run(['git','restore','--source=HEAD','--',*FILES],cwd=ROOT,check=False)
    (OUT/'summary.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
