"""Run the real stress driver with browser Boat status replies delayed 3s.

Run from the repository root; pass ordinary conversation-stress arguments.
The temporary driver copy is beside the original so all imports stay real.
Requests, authentication and response contents are forwarded unchanged.
"""
import pathlib
import subprocess
import sys
import tempfile

source = pathlib.Path('verification/desktop/scripts/conversation-stress.mjs')
text = source.read_text()
needle = 'preparePage: async (page) => {\n          await stack.vault.install(page)'
assert text.count(needle) == 1
route = '''preparePage: async (page) => {
          await page.routeWebSocket(() => true, socket => {
            const server = socket.connectToServer()
            const pending = new Set()
            const parse = message => {
              try { return JSON.parse(String(message)) } catch { return undefined }
            }
            socket.onMessage(message => {
              const frame = parse(message)
              if (frame?.type === "req" && frame.method === "boat.status")
                pending.add(frame.id)
              server.send(message)
            })
            server.onMessage(message => {
              const frame = parse(message)
              if (frame?.type === "res" && pending.delete(frame.id)) {
                console.error("latency probe: delaying unchanged Boat status reply 3000ms")
                setTimeout(() => {
                  try { socket.send(message) } catch { /* page closed */ }
                }, 3000)
              } else socket.send(message)
            })
          })
          await stack.vault.install(page)'''
with tempfile.NamedTemporaryFile(mode='w', prefix='.1218-account-delay-', suffix='.mjs',
                                 dir=source.parent, delete=False) as file:
    file.write(text.replace(needle, route))
    probe = pathlib.Path(file.name)
try:
    result = subprocess.run(['node', str(probe), *sys.argv[1:]])
finally:
    probe.unlink()
sys.exit(result.returncode)
