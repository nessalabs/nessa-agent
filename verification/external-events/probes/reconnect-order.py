#!/usr/bin/env python3
"""Probe actual client reconnect ordering; this is not a browser/stress reproduction.

Usage: python3 reconnect-order.py <base-checkout> <head-checkout>
Only import extensions are adapted for Node's TypeScript transform.
"""
from pathlib import Path
from tempfile import TemporaryDirectory
import subprocess,json,hashlib,sys
roots=[Path(root).resolve() for root in sys.argv[1:]]
assert len(roots)==2, 'pass base and head checkout paths'
files=['application/managed-session.ts','application/connection-closed-error.ts','application/connect-retry.ts','application/client-config.ts','application/rpc-error.ts','application/stage.ts','generated/product.ts']
probe='''import assert from 'node:assert/strict';
import {ManagedSession,NessaSessionUnavailableError} from './application/managed-session.ts';
import {NessaClientConfig} from './application/client-config.ts';
import {NessaConnectionClosedError} from './application/connection-closed-error.ts';
class Transport {
 listeners=new Set(); calls=[];
 request(method,params){this.calls.push({method,params});return Promise.resolve('ok');}
 onClose(fn){this.listeners.add(fn);return()=>this.listeners.delete(fn);}
 onEvent(){return()=>{};}
 close(){this.drop(1000);}
 drop(code=1006){this.termination=new NessaConnectionClosedError(code,'');for(const fn of [...this.listeners])fn(this.termination);this.listeners.clear();}
}
const first=new Transport(),replacement=new Transport();
const connected=wire=>({wire,ready:{sessionId:'local-fixture'},profile:'product',url:'ws://127.0.0.1:1/session'});
let advance;const backoff=new Promise(resolve=>advance=resolve);
const client=new ManagedSession(connected(first),new NessaClientConfig({reconnect:{jitter:false}}),async()=>connected(replacement),{wait:()=>backoff,random:()=>0});
first.drop();assert.equal(client.state.status,'reconnecting');
await assert.rejects(client.request('conversation.submit',{text:'upgrade reply'}),NessaSessionUnavailableError);
assert.equal(first.calls.length,0);assert.equal(replacement.calls.length,0);
advance();for(let i=0;i<20;i++)await Promise.resolve();
assert.equal(client.state.status,'connected');assert.equal(replacement.calls.length,0,'failed early send must not be silently replayed');
assert.equal(await client.request('conversation.submit',{text:'reply after connected'}),'ok');assert.equal(replacement.calls.length,1);
client.close();console.log(JSON.stringify({earlySend:'NessaSessionUnavailableError',earlyRpcDispatches:0,automaticReplay:0,explicitAfterConnected:'ok'}));
'''
results=[]
for root in roots:
 with TemporaryDirectory(prefix='nessa-reconnect-order-') as temporary:
  dest=Path(temporary)
  hashes={}
  for name in files:
   src=root/'packages/nessa-client/src'/name
   data=src.read_text();hashes[name]=hashlib.sha256(data.encode()).hexdigest()
   target=dest/name;target.parent.mkdir(parents=True,exist_ok=True)
   target.write_text(data.replace('.js"','.ts"'))
  (dest/'probe.mjs').write_text(probe)
  r=subprocess.run(['node','--experimental-transform-types',str(dest/'probe.mjs')],capture_output=True,text=True)
  assert r.returncode==0,(r.stdout,r.stderr)
  results.append({'checkout':str(root),'revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip(),'sourceHashes':hashes,'exit':r.returncode,'observation':json.loads(r.stdout)})
assert results[0]['sourceHashes']==results[1]['sourceHashes']
(Path(__file__).resolve().parents[1]/'baseline-reconnect-probe.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
