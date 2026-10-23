import { createServer } from "node:http"

export async function fixtureSite(port = 0) {
  const server = createServer((req, res) => {
    res.setHeader("Cache-Control", "no-store")
    if (req.url === "/download") {
      res.writeHead(200, {
        "Content-Type": "text/plain",
        "Content-Disposition": 'attachment; filename="test-only.txt"',
      })
      return res.end("nessa experiment test download\n")
    }
    res.setHeader("Content-Type", "text/html; charset=utf-8")
    res.end(`<!doctype html><html lang="en"><meta charset="utf-8"><title>Dedicated test browser</title>
<style>body{font:20px system-ui;background:#eef3fa;color:#172238;padding:48px;max-width:800px}button,input,a{font:inherit;padding:12px;margin:8px}output{display:block;padding:16px;background:white;border-radius:12px}h1{font-size:32px}</style>
<h1>Nessa dedicated browser • test fixture</h1><p>Human and agent share this page and its dedicated test storage.</p>
<button id="counter">Increment shared count</button><output id="count"></output>
<label>Shared note <input id="note" aria-label="Shared note"></label>
<p><a href="/next">Next page</a><a id="download" href="/download">Download test file</a><a target="_blank" href="/popup">Open popup</a></p>
<output id="extension">Waiting for test extension…</output><output id="route"></output>
<script>
const count=document.querySelector('#count'),note=document.querySelector('#note');
function render(){count.textContent='Shared count: '+(localStorage.getItem('count')||'0')}
render();note.value=localStorage.getItem('note')||'';
document.querySelector('#counter').onclick=()=>{localStorage.setItem('count',String(Number(localStorage.getItem('count')||0)+1));render()};
note.oninput=()=>localStorage.setItem('note',note.value);
document.querySelector('#route').textContent='Route: '+location.pathname;
</script></html>`)
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(port, "127.0.0.1", resolve)
  })
  return {
    home: `http://127.0.0.1:${server.address().port}/`,
    close: () => new Promise((resolve) => server.close(resolve)),
  }
}
