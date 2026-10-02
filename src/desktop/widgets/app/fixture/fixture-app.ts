/**
 * A fixture MCP App: a document that speaks the `ui/*` bridge by hand, as the
 * spec shows an app can without an SDK (*Transport Layer*), and puts what it
 * hears where a test or `verification/desktop/scripts/mcp-apps.mjs` can read
 * it. It is the app of the fixture server (`fixture-plugin.ts`).
 *
 * What it does, by `data-fixture` control: `call-allowed` calls a tool the
 * server lets apps call, `call-hidden` one it does not; `fetch` asks the
 * network for a page no CSP of its declares; `fullscreen` asks for the
 * fullscreen mode; `close` asks the host to tear it down, and it answers the
 * teardown a moment later, as an app saving its work would. What it heard is
 * in `data-fixture-*` attributes on its body.
 */
export const fixtureAppHtml = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Fixture app</title>
<style>
  :root { color-scheme: light dark; }
  body { margin: 0; padding: 12px; font: 13px/1.4 var(--font-sans, system-ui, sans-serif);
    color: var(--color-text-primary, CanvasText); }
  button { margin: 0 6px 6px 0; }
  output { display: block; min-height: 1.4em; }
</style>
</head>
<body data-fixture-state="loading">
<h1 style="font-size:14px;margin:0 0 8px">Fixture app</h1>
<div>
  <button data-fixture="call-allowed">Call allowed tool</button>
  <button data-fixture="call-hidden">Call hidden tool</button>
  <button data-fixture="fetch">Fetch</button>
  <button data-fixture="fullscreen">Fullscreen</button>
  <button data-fixture="close">Close</button>
</div>
<output id="input"></output>
<output id="result"></output>
<output id="call"></output>
<output id="fetch"></output>
<output id="mode"></output>
<script>
(function () {
  var parentWindow = window.parent;
  var next = 1;
  var waiting = {};
  var body = document.body;
  function post(message) { parentWindow.postMessage(message, "*"); }
  function ask(method, params) {
    var id = next++;
    post({ jsonrpc: "2.0", id: id, method: method, params: params });
    return new Promise(function (resolve) { waiting[id] = resolve; });
  }
  function tell(method, params) { post({ jsonrpc: "2.0", method: method, params: params }); }
  function show(id, text) { document.getElementById(id).textContent = text; }
  function outcome(message) {
    return message.error ? "error: " + message.error.message : "ok: " + JSON.stringify(message.result);
  }
  var context = {};
  function remember(update) {
    for (var key in update) context[key] = update[key];
    body.setAttribute("data-fixture-context", JSON.stringify(context));
    if (context.displayMode) body.setAttribute("data-fixture-mode", context.displayMode);
    if (context.theme) body.setAttribute("data-fixture-theme", context.theme);
  }
  window.addEventListener("message", function (event) {
    if (event.source !== parentWindow) return;
    var message = event.data;
    if (!message || message.jsonrpc !== "2.0") return;
    if (message.id !== undefined && !message.method) {
      var resolve = waiting[message.id];
      delete waiting[message.id];
      if (resolve) resolve(message);
      return;
    }
    switch (message.method) {
      case "ui/notifications/tool-input":
        show("input", JSON.stringify(message.params.arguments));
        body.setAttribute("data-fixture-input", JSON.stringify(message.params.arguments));
        return;
      case "ui/notifications/tool-result":
        show("result", JSON.stringify(message.params.structuredContent));
        body.setAttribute("data-fixture-result", JSON.stringify(message.params.structuredContent));
        return;
      case "ui/notifications/host-context-changed":
        remember(message.params);
        return;
      case "ui/resource-teardown":
        // Saving takes a moment; the host waits for the answer before it closes.
        body.setAttribute("data-fixture-state", "tearing-down");
        setTimeout(function () {
          body.setAttribute("data-fixture-state", "torn-down");
          post({ jsonrpc: "2.0", id: message.id, result: {} });
        }, 400);
        return;
    }
  });
  function reportSize() {
    tell("ui/notifications/size-changed", {
      width: document.documentElement.scrollWidth,
      height: document.documentElement.scrollHeight
    });
  }
  ask("ui/initialize", {
    appInfo: { name: "Fixture", version: "1.0.0" },
    appCapabilities: { availableDisplayModes: ["inline", "fullscreen"] },
    protocolVersion: "2026-01-26"
  }).then(function (answer) {
    if (answer.error) { body.setAttribute("data-fixture-state", "refused"); return; }
    remember(answer.result.hostContext);
    body.setAttribute("data-fixture-capabilities", JSON.stringify(answer.result.hostCapabilities));
    body.setAttribute("data-fixture-state", "live");
    tell("ui/notifications/initialized", {});
    reportSize();
    new ResizeObserver(reportSize).observe(body);
  });
  function on(control, run) {
    document.querySelector('[data-fixture="' + control + '"]').addEventListener("click", run);
  }
  on("call-allowed", function () {
    ask("tools/call", { name: "fixture_refresh", arguments: { times: 1 } }).then(function (m) { show("call", outcome(m)); });
  });
  on("call-hidden", function () {
    ask("tools/call", { name: "fixture_secret", arguments: {} }).then(function (m) { show("call", outcome(m)); });
  });
  on("fetch", function () {
    show("fetch", "fetching");
    fetch("https://example.com/").then(function () { show("fetch", "loaded"); }, function () { show("fetch", "blocked"); });
  });
  on("fullscreen", function () {
    ask("ui/request-display-mode", { mode: "fullscreen" }).then(function (m) { show("mode", outcome(m)); });
  });
  on("close", function () { tell("ui/notifications/request-teardown", {}); });
})();
</script>
</body>
</html>
`
