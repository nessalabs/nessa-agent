from pathlib import Path
import csv,json,collections
out=Path('/tmp/627-consolidated-benchmarks');proof={};sqlite_metadata={}
for phase in ['current','inline','current-repeat']:
 rows=list(csv.DictReader((out/f'sqlite-{phase}.csv').open()));assert len(rows)==21000
 groups=collections.Counter((r['roots'],r['closed'],r['concurrency'],r['operation']) for r in rows)
 assert len(groups)==21 and set(groups.values())=={1000}
 assert all(r['instances']=='1' and r['spawns']=='0' and int(r['elapsed_ns'])>0 for r in rows)
 meta={k:{r['body_bytes'] for r in rows if (r['roots'],r['closed'])==k} for k in {(r['roots'],r['closed']) for r in rows}}
 assert len(meta)==7 and all(len(v)==1 for v in meta.values());sqlite_metadata[phase]=meta
 batches=list(csv.DictReader((out/f'sqlite-{phase}.csv.batches.csv').open()));assert len(batches)==1750
 batchgroups=collections.Counter((r['roots'],r['closed']) for r in batches);assert len(batchgroups)==7 and set(batchgroups.values())=={250}
 assert all(r['scheduled_callers']=='4' and r['spawns']=='0' and int(r['wall_elapsed_ns'])>0 for r in batches)
 oauth=list(csv.DictReader((out/f'oauth-{phase}.csv').open()));assert len(oauth)==1000
 assert collections.Counter(r['lane'] for r in oauth)=={str(i):250 for i in range(4)}
 proof[phase]={'sqlite_original_rows':len(rows),'sqlite_scenarios':len(groups),'rows_each_scenario':1000,'batch_wall_rows':len(batches),'batch_scenarios':len(batchgroups),'batches_each_scenario':250,'oauth_flows':len(oauth),'oauth_lanes':4,'flows_each_lane':250}
assert sqlite_metadata['current']==sqlite_metadata['inline']==sqlite_metadata['current-repeat']
proof['totals']={'original_rows':66000,'batch_wall_rows':5250,'same_encoded_body_bytes_across_phases':True}
(out/'measurement-validation.json').write_text(json.dumps(proof,indent=2)+'\n');print(json.dumps(proof,indent=2))
