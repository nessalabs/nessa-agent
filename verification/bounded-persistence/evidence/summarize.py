import csv,collections,math,json
from pathlib import Path
result={}
for name in ['sqlite-worker','sqlite-inline','sqlite-worker-repeat','oauth-worker','oauth-inline','oauth-worker-repeat']:
 p=Path('/tmp/627-'+name+'.csv')
 if not p.exists():continue
 groups=collections.defaultdict(list);metadata={}
 for row in csv.DictReader(p.open()):
  keys=('roots','closed','concurrency','operation') if name.startswith('sqlite') else ('operation',)
  key=' / '.join(row[k] for k in keys);groups[key].append(int(row['elapsed_ns']));metadata[key]={k:v for k,v in row.items() if k not in ['sample','elapsed_ns','lane']}
 result[name]={}
 for k,v in groups.items():
  v.sort();result[name][k]={'n':len(v),'median_us':(v[(len(v)-1)//2]+v[len(v)//2])/2000,'p95_us':v[math.ceil(.95*len(v))-1]/1000,'p99_us':v[math.ceil(.99*len(v))-1]/1000,'metadata':metadata[k]}
Path('/tmp/627-benchmark-summary.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
