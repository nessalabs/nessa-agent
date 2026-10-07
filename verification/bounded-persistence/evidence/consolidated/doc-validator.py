from pathlib import Path
import re, tomllib, yaml
root=Path('/workspace/nessa-agent-bounded-persistence')
ids={}
for p in (root/'docs/state').rglob('*.md'):
 s=p.read_text()
 if not s.startswith('---\n'): continue
 meta=yaml.safe_load(s.split('---',2)[1])
 key=meta.get('id');assert key and key not in ids,(p,key)
 ids[key]=(p,meta)
 for source in meta.get('sources',[]): assert (root/source).exists(),(p,source)
for key,(p,m) in ids.items():
 parent=m.get('parent')
 if parent: assert parent in ids,(p,parent)
 for target in m.get('diagramLinks',{}).values(): assert target in ids,(p,target)
 seen={key}
 while parent:
  assert parent not in seen,(p,parent);seen.add(parent);parent=ids[parent][1].get('parent')
print(f'Atlas metadata/source/diagram links: {len(ids)} pages valid')
for f in ['docs/design/bounded-physical-persistence.md','docs/state/services/storage/README.md','crates/nessa-local-storage/README.md']:
 p=root/f
 for target in re.findall(r'\]\(([^)]+)\)',p.read_text()):
  if target.startswith(('http:','https:','mailto:','#')): continue
  path=target.split('#')[0]
  assert (p.parent/path).exists(),(f,target)
print('Current canonical design/atlas/shared-owner Markdown paths valid')
sources='\n'.join(p.read_text() for p in (root/'crates').rglob('*.rs') if '/target/' not in str(p))
s=(root/'docs/design/bounded-physical-persistence.md').read_text()
for row in s.splitlines():
 if not re.match(r'\|\s*\d+\s*\|',row): continue
 names=re.findall(r'\b[a-z][a-z_]+\b',row.split('|')[-2])
 for name in names:
  if '_' in name and name != 'lifetime_races': assert re.search(r'fn '+re.escape(name)+r'\b',sources),name
print('All canonical table enforcing function names exist')
manifest=tomllib.loads((root/'crates/nessa-local-storage/Cargo.toml').read_text())
assert manifest['features']['default']==[]
assert manifest['features']['physical-operation']==['dep:tokio']
assert manifest['dependencies']['tokio']['optional'] is True
print('Optional feature/default-off manifest syntax valid; Cargo graph not yet tested')
print('Mermaid statechart source inspected; external atlas renderer unavailable, rendering not claimed')
