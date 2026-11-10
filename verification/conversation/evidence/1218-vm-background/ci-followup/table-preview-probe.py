"""Replay one real older list snapshot to the table check, then read normally.

This models projection lag for the consumer; it does not claim the gateway
returned stale data in this run. No expected text or response is fabricated.
Run from the repository root with ordinary conversation-stress arguments.
"""
import pathlib
import difflib
import subprocess
import sys
import tempfile

source = pathlib.Path('verification/desktop/scripts/conversation-stress.mjs')
text = source.read_text()
start = text.index('  async table(run, step, failures) {')
end = text.index('\n  async ', start + 1)
table = text[start:end]
args = sys.argv[1:]
if '--revert-preview-wait' in args:
    args.remove('--revert-preview-wait')
    fixed = '''    let listed
    const previewed = await waitFor(async () => {
      const { conversations } = await run.client.conversation.list({})
      listed = conversations.find((each) => each.conversationId === run.main.id)
      return listed?.preview === `${SAYS.table}${text}`.trim()
    }, 10_000)
    if (!previewed)'''
    reverted = '''    const listed = await waitFor(async () => {
      const { conversations } = await run.client.conversation.list({})
      return conversations.find((each) => each.conversationId === run.main.id)
    }, 10_000)
    if (listed?.preview !== `${SAYS.table}${text}`.trim())'''
    assert table.count(fixed) == 1
    mutant = table.replace(fixed, reverted)
    sys.stderr.writelines(difflib.unified_diff(table.splitlines(True), mutant.splitlines(True),
                                            fromfile='fixed/table', tofile='reverted/table'))
    table = mutant
opening = '  async table(run, step, failures) {\n'
table = table.replace(opening, opening + '''    const retainedList = await run.client.conversation.list({})
    let heldList = true
''', 1)
read = 'const { conversations } = await run.client.conversation.list({})'
assert table.count(read) == 1
table = table.replace(read, '''const snapshot = heldList ? retainedList : await run.client.conversation.list({})
      if (heldList) console.error("projection probe: replaying one earlier real list snapshot")
      heldList = false
      const { conversations } = snapshot''')
text = text[:start] + table + text[end:]
with tempfile.NamedTemporaryFile(mode='w', prefix='.1218-preview-', suffix='.mjs',
                                 dir=source.parent, delete=False) as file:
    file.write(text)
    probe = pathlib.Path(file.name)
try:
    result = subprocess.run(['node', str(probe), *args])
finally:
    probe.unlink()
sys.exit(result.returncode)
