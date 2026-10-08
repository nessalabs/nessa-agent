#!/usr/bin/env node
/**
 * MCP Apps (ADR 344, #349), through the fixture app the sample workspace
 * registers (`src/desktop/widgets/app/fixture/`): it renders inline, in a pane
 * (its own fullscreen request) and in the window, each place told its
 * display mode and given its tool call; `tools/call` is answered for an
 * allowed tool and refused for a hidden one; `ui/message` lands in the
 * transcript labelled with the app that wrote it, and a context is refused
 * as the sample has no model (#390); a request to an origin its CSP
 * does not declare is blocked, and the host says so; the app runs on an
 * opaque origin that reaches neither the window nor storage; it cannot leave
 * its frame for a page without its policy, nor speak as the proxy; the
 * sandbox proxy, handed documents the host's own builder writes, reads every
 * way the app's document is replaced as its departure and nothing else as
 * one; the test server's chart page handshakes through that proxy and draws
 * the tool result it is sent; and it is torn down on close — by its pane's
 * close, and by its own request.
 *
 * The app's documents are cross-origin to the page, so the script reads them
 * through Playwright's frames, never through the page.
 *
 * Every check runs on a fresh page, in each engine and layout.
 */
import { mkdirSync } from "node:fs"
import { createServer } from "node:http"
import { join } from "node:path"
import {
  CHART_URI,
  answer as mcpAnswer,
} from "../../../scripts/mcp-test-server/server.mjs"
import { attempt, CannotRun, devServerOnlySteps, recordIfLeftOut } from "./lib/cli.mjs"
import { appFrame } from "./lib/apps.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { target } from "./lib/server.mjs"
import { content, css, names, selectorFor } from "./lib/selectors.mjs"
import { contentIs, paneCount, paneCountIs, settled, until } from "./lib/workspace.mjs"

const meta = {
  name: "mcp-apps",
  summary:
    "MCP Apps: places, tools/call, CSP, isolation, escapes, forgery, departures, the chart app, teardown",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/mcp-apps.mjs [options]

Checks, per engine and layout (--only <names> to pick):
  inline       the card's app is live, told inline, its call's input and result,
               and its frame as tall as the app said
  pane         the app's fullscreen request opens a pane beside the conversation,
               whose app is live and told fullscreen; the card's stays inline
  window       the pane's Open in Window shows the app in the window, live,
               told fullscreen, filling it
  tools-call   tools/call answered for the allowed tool, refused for the hidden one
  message      ui/message answered {} and landing in the transcript as one message
               of the person's with the app's words, labelled "Sent by
               show_fixture, from nessa-fixture" — wrapped, not cut, so with no
               title; each name in a <bdi>; data-message-app server/tool —
               above its bubble, over the
               bubble's right edge (within 6 px), inside the column, and no label
               on the person's own; another while the sample's reply runs is
               isError and adds nothing; ui/update-model-context is refused, the
               sample having no model to give it to (#390)
  csp          a fetch to an undeclared origin is blocked, and the host's notice
               names it
  isolation    the app's origin is opaque: no parent document, no storage; the
               proxy is on another origin than the window
  escape-navigate, escape-refresh, escape-rewrite
               the app sending its frame away, by script, by meta refresh, or
               after rewriting its document (which erases its reporter):
               refused (nothing reaches the other site); and either the app
               stays, live, in its own document (WebKit refuses the
               navigation before it leaves), or the frame is taken off the
               page and the host says it cannot show the app — within 5 s, or
               past the initialize deadline after a rewrite
  departures   the real proxy, in the host's frame, handed documents the host's
               own builder writes with the host's deadline (dev server only):
               another document in the frame — a reload, before or after the
               first load, a rewrite (closed or then sent away), a navigation
               or about:blank (even after the app dispatched checks of its
               own making), a document with no reporter or one answering the
               check without the token or from a frame of its own — is the
               app's departure, said once, nothing relayed after it; an app
               left alone (which never hears the check, even having patched
               the event APIs), its links to a fragment of any kind and its
               moves to one by script, a first load held by an image the
               fixture leaves unanswered, an app forging departures, and a
               third party forging them and the check's answers at every
               frame, are not; a host reply with no method of its own, after the
               app has defined method and params on Object.prototype, is not a
               check, and that app's departure is still reported; a deadline
               no timer can wait loads nothing. A javascript: rewrite does not
               run in Chromium; in WebKit it runs, then the unanswered load
               is a departure
  departures-back
               on a page of its own: going back across a move to a fragment
               stays in the document, though the frame loads again, and the
               page lives on; an answer to an earlier check, coming after a
               later load, ends no wait
  chart        the test server's chart page, handed to the real proxy as the host
               writes a document (dev server only): it reaches live, and #chart
               shows the series from the tool result it was sent
  forge        forged proxy messages change nothing; a forged report puts no
               words of the app's in the host's chrome
  teardown     closing the pane takes its frames off the page; the app asking to
               go is sent ui/resource-teardown before its pane closes

departures, departures-back and chart read the dev server's modules. Under
--mode prod those steps are not run. The other fixtures need the sandbox
proxy, which Vite starts only in configureServer, so a production preview
has no origin to mount the app. run-all --mode prod leaves this check out.
Run on its own as --mode prod, it says it could not run, before building a
preview.`,
}

/** The chart page `resources/read` serves, and the series this server's rows draw. */
const chartHtml = mcpAnswer({
  jsonrpc: "2.0",
  id: 1,
  method: "resources/read",
  params: { uri: CHART_URI },
}).result.contents[0].text
const chartSeries = "alpha 10, beta 20"

/** Says hello to the host, as an app would, each time a document of its runs. */
const hello = `parent.postMessage({ jsonrpc: "2.0", method: "hello-from-app" }, "*");`

/** Path of the image `holds-first-load` leaves unanswered. */
const holdPath = "/.nessa/hold-first-load"

/** A 1×1 gif, written only after the check has read the proxy. */
const holdGif = Buffer.from(
  "R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7",
  "base64",
)

/**
 * Listens on 127.0.0.1 and does not answer `holdPath` until `answer`. An
 * image in the app's document pointed at that URL holds the document's
 * first load for as long as the socket stays unanswered. `answer` writes
 * the gif with `connection: close` so the listener can shut without waiting
 * on a keep-alive the page still holds.
 */
function openHeldLoad() {
  /** @type {import("node:http").ServerResponse[]} */
  const pending = []
  let hits = 0
  const server = createServer((request, response) => {
    if (request.url?.split("?")[0] !== holdPath) {
      response.writeHead(404)
      response.end()
      return
    }
    hits += 1
    pending.push(response)
    request.on("close", () => {
      if (!response.writableEnded) response.destroy()
    })
  })
  return new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", () => {
      const address = server.address()
      const port = typeof address === "object" && address ? address.port : 0
      resolve({
        origin: `http://127.0.0.1:${port}`,
        hits: () => hits,
        unanswered: () => pending.filter((response) => !response.writableEnded).length,
        answer() {
          for (const response of pending) {
            if (response.writableEnded) continue
            response.writeHead(200, {
              "content-type": "image/gif",
              "content-length": String(holdGif.length),
              "cache-control": "no-store",
              connection: "close",
            })
            response.end(holdGif)
          }
        },
        close() {
          return new Promise((done) => {
            const timer = setTimeout(() => {
              for (const response of pending) response.socket?.destroy()
            }, 1000)
            server.close(() => {
              clearTimeout(timer)
              done()
            })
          })
        },
      })
    })
  })
}

/**
 * After the document loads, follows each link \`[id, fragment]\` names — a
 * click as a person makes one — and says \`moved-<id>\` when the document
 * moved to that fragment and is still this one.
 */
const followAfterLoad = (...links) =>
  `addEventListener("load", function () { setTimeout(function () {
    ${links
      .map(
        ([id, fragment]) => `
    var link = document.getElementById(${JSON.stringify(id)}) || document.getElementById("host").shadowRoot.getElementById(${JSON.stringify(id)});
    link.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, composed: true, button: 0 }));
    if (location.href === "about:srcdoc${fragment}")
      parent.postMessage({ jsonrpc: "2.0", method: "moved-${id}" }, "*");`,
      )
      .join("")}
    ${hello}
  }, 200); });`

/** The links \`fragment-links\` follows: \`[id, the URL's fragment after it]\`. */
const fragmentLinks = [
  ["plain", "#x"],
  ["empty", "#"],
  ["self", "#v"],
  ["spaced", "#u"],
  ["svg", "#y"],
  ["area", "#z"],
  ["shadowed", "#w"],
]

/**
 * Documents for the \`departures\` check, by name: the HTML handed to the
 * proxy (behind the host's own policy and reporter, unless \`bare\`); whether
 * the proxy must report the app gone; what the app must have said
 * (\`must\`), its premise, and what must never reach the host (\`never\`,
 * by engine where the engines differ);
 * and the deadline handed over, when not the host's. \`@appLeft@\`,
 * \`@appCheck@\` and \`@slot@\` are written, in the page, from the names
 * \`sandbox-methods.ts\` states, never spelled here.
 */
const departureScenarios = {
  // Also: the app never hears the proxy's check.
  "left-alone": {
    html: `<p>quiet</p><script>${hello}addEventListener("message", function (event) {
      if (event.source === parent && event.data && event.data.method === "@appCheck@")
        parent.postMessage({ jsonrpc: "2.0", method: "heard-the-check" }, "*");
    })</script>`,
    leaves: false,
  },
  "forged-departures": {
    html: `<script>${hello}setTimeout(function () {
      parent.postMessage({ jsonrpc: "2.0", method: "@appLeft@", params: {} }, "*");
      parent.postMessage({ jsonrpc: "2.0", method: "@appLeft@", params: { token: "@slot@" } }, "*");
      parent.postMessage({ jsonrpc: "2.0", method: "@appLeft@", params: { token: "0123456789abcdef0123456789abcdef" } }, "*");
    }, 300)</script>`,
    leaves: false,
  },
  // Links to a fragment of the document stay in it (its base URL is the
  // proxy's), whatever kind of link, wherever it is.
  "fragment-links": {
    html: `<a id="plain" href="#x">x</a><a id="empty" href="#">top</a>
      <a id="self" href="#v" target="_self">v</a><a id="spaced" href=" #u">u</a>
      <svg width="20" height="20"><a id="svg" href="#y"><rect width="20" height="20"/></a></svg>
      <map name="m"><area id="area" href="#z" shape="rect" coords="0,0,10,10"></map>
      <img usemap="#m" width="10" height="10" alt="">
      <div id="host"></div><p id="x">x</p>
      <script>${hello}document.getElementById("host").attachShadow({ mode: "open" }).innerHTML = '<a id="shadowed" href="#w">w</a>';
      ${followAfterLoad(...fragmentLinks)}</script>`,
    leaves: false,
    must: fragmentLinks.map(([id]) => `moved-${id}`),
  },
  // A move to a fragment by script stays in the document. (WebKit fires a
  // load on the frame for it: a load alone is not a departure.)
  "fragment-by-script": {
    html: `<script>${hello}addEventListener("load", function () { setTimeout(function () {
      location.hash = "#y";
      if (location.href === "about:srcdoc#y") parent.postMessage({ jsonrpc: "2.0", method: "moved" }, "*");
    }, 200); });</script>`,
    leaves: false,
    must: ["moved"],
  },
  "rewrite-alone": {
    html: `<script>${hello}setTimeout(function () { document.open(); document.write("<p>rewritten</p>"); document.close(); }, 300)</script>`,
    leaves: true,
  },
  // The reloaded document names itself, before any of its own scripts: it
  // is another document, and none of it is relayed.
  "rewrite-then-reload": {
    html: `<script>if (window.name) parent.postMessage({ jsonrpc: "2.0", method: "hello-again" }, "*");
      else { ${hello}window.name = "rewritten"; setTimeout(function () { document.open(); document.write("<p>w</p>"); document.close(); setTimeout(function () { location.reload(); }, 200); }, 300); }</script>`,
    leaves: true,
    never: ["hello-again"],
  },
  "rewrite-then-navigate": {
    html: `<script>${hello}setTimeout(function () { document.open(); document.write("<p>w</p>"); document.close(); setTimeout(function () { location = "https://example.com/?leak=rewrite"; }, 200); }, 300)</script>`,
    leaves: true,
  },
  "rewrite-then-blank": {
    html: `<script>${hello}setTimeout(function () { document.open(); setTimeout(function () { location = "about:blank"; }, 200); }, 300)</script>`,
    leaves: true,
  },
  "navigate-before-load": {
    html: `<script>${hello}location.replace("about:blank")</script>`,
    leaves: true,
  },
  "reload-before-load": {
    html: `<script>${hello}if (!window.name) { window.name = "reloaded"; location.reload(); }</script>`,
    leaves: true,
  },
  // An image the fixture leaves unanswered (`openHeldLoad`) is in the
  // markup, so the parser starts it before the document can finish. A
  // script that inserts the image can run after Chromium has already
  // fired `load` for a document that had no outstanding resource; that
  // load is not a departure, and it is not this hold. `holding` is said
  // only while `readyState` is not `complete`. `loaded` or `not-held`
  // means that hold was not established. The check counts the socket
  // still unanswered after the wait.
  "holds-first-load": {
    html: `<img src="@hold@" alt=""><script>${hello}addEventListener("load", function () { parent.postMessage({ jsonrpc: "2.0", method: "loaded" }, "*"); });
      if (document.readyState !== "complete")
        parent.postMessage({ jsonrpc: "2.0", method: "holding" }, "*");
      else parent.postMessage({ jsonrpc: "2.0", method: "not-held" }, "*");
    </script>`,
    hold: true,
    leaves: false,
    must: ["hello-from-app", "holding"],
    never: ["loaded", "not-held"],
  },
  // What the app posts as its document goes, after its reporter's word:
  // never relayed.
  "posts-as-it-goes": {
    html: `<script>${hello}addEventListener("pagehide", function () { ${hello} });
      setTimeout(function () { location = "about:blank"; }, 300)</script>`,
    leaves: true,
  },
  // The app has its reporter answer checks of its own making, for every
  // check number to come, behind enough of its own messages to hold the
  // proxy's queue up past its next load; then sends its frame to a blank
  // page with the reporter erased. None is answered — events the app
  // dispatches are not trusted — so the blank page's load goes unanswered.
  // (WebKit delivers such answers after the blank page's load: Chromium
  // passes either way. 20 × 8 MiB is about the least that held WebKit's
  // queue long enough here; 10 × 4 MiB did not.)
  "forged-checks": {
    html: `<script>${hello}addEventListener("load", function () { setTimeout(function () {
      var big = "x".repeat(8 * 1024 * 1024);
      for (var j = 0; j < 20; j++) parent.postMessage({ jsonrpc: "2.0", method: "@reserved@junk", params: { b: big } }, "*");
      for (var n = 1; n <= 30; n++)
        dispatchEvent(new MessageEvent("message", { source: parent, data: { method: "@appCheck@", params: { check: n } } }));
      document.open();
      location.replace("about:blank");
    }, 200); });</script>`,
    leaves: true,
  },
  // A host reply has no method of its own (`{ id, result }`). The app lends
  // `method` and `params` from Object.prototype, then opens and closes its
  // document. That fires the frame's next load synchronously, during the
  // call, and the load is check 2. (`location.replace("about:blank")` fires
  // its load after this turn, so an answer posted in the turn meets it too
  // late to keep the frame.) The reporter reads only the message's own
  // data, so it never calls the getter and does not stop the app's
  // listener: the listener is what opens the document, nothing answers
  // check 2, and the frame leaves. A reporter that reads `data.method` and
  // `data.params` does call the getter, which opens the document and
  // returns check 2; the reporter then posts that answer, the load is
  // already check 2, and the frame stays.
  "prototype-reply": {
    html: `<script>
      Object.prototype.method = "@appCheck@";
      var opened = false;
      function openDocument() {
        if (opened) return;
        opened = true;
        document.open();
        document.close();
      }
      Object.defineProperty(Object.prototype, "params", {
        configurable: true,
        get: function () {
          openDocument();
          return { check: 2 };
        }
      });
      ${hello}
      addEventListener("message", function (event) {
        var data = event.data;
        if (!data || typeof data !== "object" || !Object.hasOwn(data, "result")) return;
        openDocument();
      });
      addEventListener("load", function () { setTimeout(function () {
        parent.postMessage({ jsonrpc: "2.0", method: "send-host-reply" }, "*");
      }, 200); });
    </script>`,
    replies: 1,
    leaves: true,
    must: ["send-host-reply"],
  },
  // As \`forged-checks\`, with the checks real messages from a frame of the app's own,
  // \`source\` patched to say the proxy: the reporter reads the source it
  // took before the app ran, and answers none.
  "forged-checks-from-a-frame": {
    html: `<script>${hello}addEventListener("load", function () { setTimeout(function () {
      var big = "x".repeat(8 * 1024 * 1024);
      for (var j = 0; j < 20; j++) parent.postMessage({ jsonrpc: "2.0", method: "@reserved@junk", params: { b: big } }, "*");
      Object.defineProperty(MessageEvent.prototype, "source", { get: function () { return parent; } });
      var frame = document.createElement("iframe");
      frame.srcdoc = "<scr" + "ipt>for (var n = 1; n <= 30; n++) parent.postMessage({ method: '@appCheck@', params: { check: n } }, '*');</scr" + "ipt>";
      frame.onload = function () { setTimeout(function () { document.open(); location.replace("about:blank"); }, 50); };
      document.body.appendChild(frame);
    }, 200); });</script>`,
    leaves: true,
  },
  // An app that replaces what the reporter would look up on the event — its
  // source, and how it is stopped — still never hears the proxy's check.
  "patched-app": {
    html: `<script>${hello}var realSource = Object.getOwnPropertyDescriptor(MessageEvent.prototype, "source").get;
      Event.prototype.stopImmediatePropagation = function () {};
      Object.defineProperty(MessageEvent.prototype, "source", { get: function () { return parent; } });
      addEventListener("message", function (event) {
        if (realSource.call(event) === parent && event.data && event.data.method === "@appCheck@")
          parent.postMessage({ jsonrpc: "2.0", method: "heard-the-check" }, "*");
      });</script>`,
    leaves: false,
  },
  // A deadline no timer can wait: the proxy loads nothing.
  "unusable-deadline": {
    html: `<script>${hello}</script>`,
    checkWithin: Infinity,
    leaves: false,
    never: ["hello-from-app"],
  },
  "no-reporter": {
    html: `<p>a document the host did not write</p>`,
    bare: true,
    leaves: true,
  },
  // A document with no reporter that knows the protocol: it answers the
  // check, without the token.
  impostor: {
    html: `<script>addEventListener("message", function (event) {
      if (event.data && event.data.method === "@appCheck@") {
        parent.postMessage({ jsonrpc: "2.0", method: "@appCheck@", params: {} }, "*");
        parent.postMessage({ jsonrpc: "2.0", method: "@appCheck@", params: { token: "0123456789abcdef0123456789abcdef", document: "impostor" } }, "*");
      }
    })</script>`,
    bare: true,
    leaves: true,
  },
  // A document with no reporter whose own frame holds the token — the slot
  // is filled wherever it first is — and answers from there: not the app's
  // frame, so not heard.
  "nested-answers": {
    html: `<script>var frame = document.createElement("iframe");
      frame.srcdoc = "<scr" + "ipt>setInterval(function () { parent.parent.postMessage({ jsonrpc: '2.0', method: '@appCheck@', params: { token: '@slot@', document: 'nested' } }, '*'); }, 200)</scr" + "ipt>";
      document.documentElement.appendChild(frame);</script>`,
    bare: true,
    leaves: true,
  },
  // \`location.href = "javascript:…"\` that opens and rewrites the document.
  // The policy allows inline script, so this is the same class of attempt as
  // \`document.open()\`. Measured: Chromium does not run the URL (nothing it
  // posts, no second load, not a departure, the app still there). WebKit runs
  // it — its post is relayed — and the frame then loads; the pinned document
  // does not answer, so the departure follows. The app does not see that load
  // (\`loaded-again\` is neither engine's). The WebKit post before the
  // departure is the same window as any navigation not yet judged (#388).
  "javascript-url": {
    html: `<p id="stay">here</p><script>${hello}
      var loads = 0;
      addEventListener("load", function () {
        loads += 1;
        if (loads > 1) parent.postMessage({ jsonrpc: "2.0", method: "loaded-again" }, "*");
      });
      setTimeout(function () {
        location.href = "javascript:document.open();document.write('<p>replaced</p>');document.close();parent.postMessage({jsonrpc:'2.0',method:'replaced'},'*')";
        setTimeout(function () {
          parent.postMessage({ jsonrpc: "2.0", method: "still-here" }, "*");
        }, 400);
      }, 300);
    </script>`,
    leaves: { chromium: false, webkit: true },
    must: ["hello-from-app", "still-here"],
    never: { chromium: ["replaced", "loaded-again"], webkit: ["loaded-again"] },
  },
}

/**
 * \`departures-back\`, on a page of its own: history is the page's joint
 * session history, so no other frame's move may come between. (The two go
 * back seconds apart.)
 */
const backScenarios = {
  // Going back across a move to a fragment stays in the document, though
  // the engines fire a load on the frame for it; and the page lives on.
  "hash-then-back": {
    html: `<script>${hello}addEventListener("hashchange", function () {
      if (location.href === "about:srcdoc") parent.postMessage({ jsonrpc: "2.0", method: "went-back" }, "*");
    });
    addEventListener("load", function () { setTimeout(function () {
      location.hash = "#/a";
      setTimeout(function () { history.back(); }, 200);
    }, 200); }, { once: true });</script>`,
    leaves: false,
    must: ["went-back"],
  },
  // A document that answers the first check only after the frame has loaded
  // again: that answer ends no later wait, and the later checks, unanswered,
  // are the app's departure. (Its token is its own: the proxy fills the
  // first slot, which is its.)
  "stale-answer": {
    html: `<script>
      var token = "@slot@";
      var say = function (params) { parent.postMessage({ jsonrpc: "2.0", method: "@appCheck@", params: params }, "*"); };
      say({ token: token, document: "stale" });
      addEventListener("message", function (event) {
        if (event.source !== parent || !event.data || event.data.method !== "@appCheck@") return;
        if (event.data.params.check !== 1) return;
        setTimeout(function () {
          location.hash = "#a";
          setTimeout(function () { history.back(); }, 200);
        }, 3000);
        setTimeout(function () {
          say({ token: token, document: "stale", check: 1 });
          parent.postMessage({ jsonrpc: "2.0", method: "answered-late" }, "*");
        }, 4500);
      });
    </script>`,
    bare: true,
    leaves: true,
    must: ["answered-late"],
  },
}

/**
 * Drives the real proxy, in the host's own frame, with \`scenarios\`' documents
 * from the host's own builder and deadline, and a third party forging at
 * every frame; judges what each proxy said against its scenario.
 */
async function departuresOn(page, scenarios) {
  const failures = []
  const leaked = []
  page.on("request", (request) => {
    if (new URL(request.url()).hostname === "example.com") leaked.push(request.url())
  })
  let crashed = false
  page.on("crash", () => (crashed = true))
  // The dev server's modules, and its sandbox, are what this check begins
  // from (a preview answers any path with its page); past this, anything
  // that throws is the check failing.
  const dev = await page.evaluate(async () => {
    const answer = await fetch("/src/desktop/widgets/app/model/csp.ts")
    const type = answer.headers.get("content-type") ?? ""
    return answer.ok && type.includes("javascript")
  })
  if (!dev) throw new CannotRun("departures needs the dev server's modules")
  const sandboxed = await page.evaluate(async () => {
    const origin = await import("/src/desktop/widgets/app/adapters/dom/sandbox-origin.ts")
    return origin.pageSandbox(document) !== undefined
  })
  if (!sandboxed) throw new CannotRun("the page names no sandbox it may use")
  const count = Object.keys(scenarios).length
  const needsHold = Object.values(scenarios).some((scenario) => scenario.hold === true)
  // Open before the documents run, and answer only after their messages are
  // in hand: the unanswered image is the first-load hold.
  const heldLoad = needsHold ? await openHeldLoad() : undefined
  /** @type {{ hits: number, unanswered: number } | undefined} */
  let held
  let seen
  try {
    seen = await page
      .evaluate(
        async ({ scenarios, holdOrigin, holdUrl }) => {
          const csp = await import("/src/desktop/widgets/app/model/csp.ts")
          const {
            sandboxMethods: methods,
            frameTokenSlot,
            sandboxPrefix,
          } = await import("/src/desktop/widgets/app/model/sandbox-methods.ts")
          const origin =
            await import("/src/desktop/widgets/app/adapters/dom/sandbox-origin.ts")
          const { deadlines } =
            await import("/src/desktop/widgets/app/application/bridge.ts")
          const sandbox = origin.pageSandbox(document)
          const named = (html) =>
            html
              .replaceAll("@appLeft@", methods.appLeft)
              .replaceAll("@appCheck@", methods.appCheck)
              .replaceAll("@reserved@", sandboxPrefix)
              .replaceAll("@slot@", frameTokenSlot)
              .replaceAll("@hold@", holdUrl)
          const seen = {}
          const replies = {}
          const proxies = []
          let handed = 0
          for (const [name, { html, bare, checkWithin, hold }] of Object.entries(
            scenarios,
          )) {
            // The host's side: its frame, as the app view makes it, and the
            // document, policy and deadline the bridge hands over. A held
            // first load may fetch the one image origin the fixture owns;
            // the policy is still the host's (`appliedCsp`, `cspPolicy`).
            const applied = csp.appliedCsp(
              hold && holdOrigin ? { csp: { resourceDomains: [holdOrigin] } } : {},
            )
            const policy = csp.cspPolicy(applied)
            const proxy = document.createElement("iframe")
            proxy.setAttribute("sandbox", origin.proxyFrameSandbox)
            // On screen, as the host's are: an engine may throttle the timers
            // of a frame it cannot see.
            proxy.style.cssText = `position:fixed;z-index:9999;left:${(proxies.length % 10) * 42}px;top:${Math.floor(proxies.length / 10) * 32}px;width:40px;height:30px`
            proxy.src = sandbox.url
            seen[name] = []
            addEventListener("message", (event) => {
              if (event.source !== proxy.contentWindow) return
              const method = event.data?.method
              if (method === methods.proxyReady) {
                proxy.contentWindow.postMessage(
                  {
                    jsonrpc: "2.0",
                    method: methods.resourceReady,
                    params: {
                      html: bare ? named(html) : csp.appDocument(named(html), applied),
                      policy,
                      checkWithin: checkWithin ?? deadlines.initialize,
                    },
                  },
                  sandbox.origin,
                )
                handed += 1
                return
              }
              // A host reply with no method of its own, after the app has
              // asked: one per check number the scenario names. The proxy
              // relays it; the app's prototype cannot make it a check.
              if (method === "send-host-reply") {
                const count = scenarios[name].replies ?? 0
                for (let i = 0; i < count; i += 1)
                  proxy.contentWindow.postMessage(
                    { jsonrpc: "2.0", id: 1, result: {} },
                    sandbox.origin,
                  )
                replies[name] = count
              }
              seen[name].push(method ?? "?")
            })
            proxies.push(proxy)
            document.body.append(proxy)
          }
          // Every document handed over, and every app that stays has spoken:
          // its frame is made.
          const speaking = Object.entries(scenarios)
            .filter(([, s]) => !s.bare && s.checkWithin === undefined)
            .map(([name]) => name)
          const started = performance.now()
          while (
            (handed < proxies.length ||
              speaking.some((name) => seen[name].length === 0)) &&
            performance.now() - started < 20_000
          )
            await new Promise((resolve) => setTimeout(resolve, 100))
          const spoke = performance.now()
          // A third party, once every app's frame is made: a frame of the
          // page's own, on no origin, that forges departures and check
          // answers at every proxy and every app frame it can reach.
          const third = document.createElement("iframe")
          third.setAttribute("sandbox", "allow-scripts")
          third.style.cssText = "position:fixed;left:-9999px"
          third.srcdoc = `<script>
          var proxies = 0, apps = 0;
          var forged = [
            { jsonrpc: "2.0", method: ${JSON.stringify(methods.appLeft)}, params: { token: "00000000000000000000000000000000" } },
            { jsonrpc: "2.0", method: ${JSON.stringify(methods.appLeft)}, params: {} },
            { jsonrpc: "2.0", method: ${JSON.stringify(methods.appCheck)}, params: { token: "00000000000000000000000000000000" } }
          ];
          for (var i = 0; i < top.frames.length; i++) {
            var w = top.frames[i];
            if (w === window) continue;
            proxies++;
            var app = null;
            try { app = w.frames[0] || null; } catch (error) { app = null; }
            if (app) apps++;
            forged.forEach(function (m) { w.postMessage(m, "*"); if (app) app.postMessage(m, "*"); });
          }
          parent.postMessage({ proxies: proxies, apps: apps }, "*");
        </script>`
          let forgedAt = { proxies: 0, apps: 0 }
          addEventListener("message", (event) => {
            if (event.source === third.contentWindow) forgedAt = event.data
          })
          document.body.append(third)
          // Then past the proxy's deadline for the latest load it waits on:
          // the rewrites come a moment after their apps first speak.
          const wait = deadlines.initialize + 6000
          await new Promise((resolve) =>
            setTimeout(resolve, Math.max(0, spoke + wait - performance.now())),
          )
          return { seen, appLeft: methods.appLeft, forgedAt, handed, replies }
        },
        {
          scenarios,
          holdOrigin: heldLoad?.origin ?? "",
          holdUrl: heldLoad ? `${heldLoad.origin}${holdPath}` : "",
        },
      )
      .catch((error) => {
        if (crashed) return null
        throw error
      })
    if (heldLoad) held = { hits: heldLoad.hits(), unanswered: heldLoad.unanswered() }
  } finally {
    if (heldLoad) {
      heldLoad.answer()
      await heldLoad.close()
    }
  }
  if (crashed || seen === null) {
    failures.push("the page crashed")
    return { failures }
  }
  if (seen.handed < count)
    failures.push(`only ${seen.handed} of ${count} proxies were ready`)
  // The third party reached every proxy, and the app frame in each that
  // still had one: those that stay, at least.
  // (A scenario whose deadline the proxy refuses has no app frame.)
  const engineName = page.context().browser().browserType().name()
  const staying = Object.values(scenarios).filter((scenario) => {
    if (scenario.checkWithin !== undefined) return false
    const leaves = scenario.leaves
    return typeof leaves === "boolean" ? !leaves : leaves[engineName] !== true
  }).length
  if (seen.forgedAt.proxies < count || seen.forgedAt.apps < staying)
    failures.push(
      `the third party forged at ${seen.forgedAt.proxies} proxies and ${seen.forgedAt.apps} apps`,
    )
  for (const [
    name,
    { leaves, must = [], never = [], replies: wantReplies },
  ] of Object.entries(scenarios)) {
    const said = seen.seen[name]
    const engine = page.context().browser().browserType().name()
    for (const wrong of Array.isArray(never) ? never : (never[engine] ?? []))
      if (said.includes(wrong))
        failures.push(`${name}: said ${wrong} (${said.join(" ")})`)
    for (const premise of must)
      if (!said.includes(premise))
        failures.push(`${name}: never said ${premise} (${said.join(" ")})`)
    const at = said.indexOf(seen.appLeft)
    // `leaves` is that engine's answer when the engines disagree.
    const leave = typeof leaves === "boolean" ? leaves : leaves[engine] === true
    if (!leave && at !== -1) failures.push(`${name}: departed (${said.join(" ")})`)
    if (leave && at === -1) failures.push(`${name}: no departure (${said.join(" ")})`)
    if (at !== -1 && said.lastIndexOf(seen.appLeft) !== at)
      failures.push(`${name}: departed more than once`)
    if (at !== -1 && said.slice(at + 1).includes("hello-from-app"))
      failures.push(
        `${name}: the app was relayed after its departure (${said.join(" ")})`,
      )
    if (said.includes("heard-the-check"))
      failures.push(`${name}: the app heard the check`)
    if (wantReplies !== undefined && (seen.replies?.[name] ?? 0) !== wantReplies)
      failures.push(
        `${name}: sent ${seen.replies?.[name] ?? 0} host replies, not ${wantReplies}`,
      )
  }
  if (leaked.length > 0)
    failures.push(`requests reached example.com: ${leaked.join(", ")}`)
  if (needsHold) {
    const requests = held?.hits ?? 0
    const open = held?.unanswered ?? 0
    if (requests < 1 || open < 1) {
      const heldNames = Object.entries(scenarios)
        .filter(([, scenario]) => scenario.hold === true)
        .map(([name]) => name)
      failures.push(
        `${heldNames.join(", ")}: the first load was not held (${requests} requests, ${open} unanswered)`,
      )
    }
  }
  return {
    seen: seen.seen,
    forgedAt: seen.forgedAt,
    leaked,
    failures,
    ...(held ? { held } : {}),
  }
}

/** A fresh page on the sample session whose conversation carries the fixture app. */
async function onSample(browser, { url, layout }) {
  const opened = await openPage(browser, { url, layout })
  const { page } = opened
  // By the session list, not the switcher's chord: what is checked is the
  // app, on any host's keyboard.
  await page.getByText(names.widgetChannel, { exact: true }).first().click()
  const row = page.getByText(names.appSession, { exact: true }).first()
  await row.waitFor({ timeout: 3000 }).catch(() => {})
  // The sidebar lists a channel's latest few; the rest are behind Show all.
  for (const more of await page.getByText(names.showAll).all()) {
    if (await row.count()) break
    await more.click()
  }
  if (!(await row.count())) throw new CannotRun(`no session row "${names.appSession}"`)
  await row.click()
  await need(page, selectorFor.appFrameIn("inline"), "the app's card frame", 10_000)
  await settled(page)
  return opened
}

/** Waits until the app in `frame` says it is `state` (`data-fixture-state`), and says what it is. */
async function appState(frame, state, timeout = 10_000) {
  try {
    await frame.waitForSelector(selectorFor.fixtureState(state), { timeout })
  } catch {
    // Reported below.
  }
  return frame.evaluate(
    (says) =>
      Object.fromEntries(
        Object.entries(says).map(([field, attribute]) => [
          field,
          document.body.getAttribute(attribute),
        ]),
      ),
    names.fixtureSays,
  )
}

/** What an output of the fixture app says, once it says anything but `pending`. */
async function output(frame, id, pending = "", timeout = 5000) {
  await frame
    .waitForFunction(
      ([id, pending]) => {
        const text = document.querySelector(id)?.textContent ?? ""
        return text !== "" && text !== pending
      },
      [id, pending],
      { timeout },
    )
    .catch(() => {})
  return frame.evaluate((id) => document.querySelector(id)?.textContent ?? "", id)
}

const rect = (element) =>
  element.evaluate((e) => {
    const r = e.getBoundingClientRect()
    return { x: r.left, y: r.top, w: r.width, h: r.height }
  })

/**
 * The place's body and the app's frame in it, measured together once the
 * place has settled (a pane opening animates its width): whether the frame
 * fills the body, within a pixel.
 */
async function fills(page, body, frame) {
  const measure = () =>
    page.evaluate(
      ([body, frame]) => {
        const r = (sel) => {
          const box = document.querySelector(sel)?.getBoundingClientRect()
          return box && { x: box.left, y: box.top, w: box.width, h: box.height }
        }
        return { body: r(body), frame: r(frame) }
      },
      [body, frame],
    )
  await until(
    page,
    ([body, frame]) => {
      const a = document.querySelector(body)?.getBoundingClientRect()
      const b = document.querySelector(frame)?.getBoundingClientRect()
      return (
        !!a &&
        !!b &&
        Math.abs(a.width - b.width) <= 1 &&
        Math.abs(a.height - b.height) <= 1
      )
    },
    [body, frame],
  ).catch(() => {})
  const both = await measure()
  const fit =
    !!both.body &&
    !!both.frame &&
    Math.abs(both.body.w - both.frame.w) <= 1 &&
    Math.abs(both.body.h - both.frame.h) <= 1
  return { ...both, fit }
}

/** Checks the app in `place` is live, told `mode`, with its call's input and result. */
async function expectLive(page, place, mode, failures) {
  const { element, proxy, app } = await appFrame(page, place)
  const said = await appState(app, "live")
  if (said.state !== "live") failures.push(`the ${place} app is ${said.state}, not live`)
  if (said.mode !== mode)
    failures.push(`the ${place} app is told ${said.mode}, not ${mode}`)
  if (said.input !== '{"title":"Fixture"}')
    failures.push(`the ${place} app was told input ${said.input}`)
  if (said.result !== '{"rows":3}')
    failures.push(`the ${place} app was told result ${said.result}`)
  const box = await rect(element)
  if (box.w <= 0 || box.h <= 0)
    failures.push(`the ${place} frame has no room: ${JSON.stringify(box)}`)
  return { element, proxy, app, said, box }
}

/** Opens the app in a pane, by its own fullscreen request from the card. */
async function openPane(page, failures) {
  const before = await paneCount(page)
  const { app } = await appFrame(page, "inline")
  await appState(app, "live")
  await app.click(selectorFor.fixtureControl("fullscreen"))
  await paneCountIs(page, before + 1)
  const answer = await output(app, selectorFor.fixtureOutput("mode"))
  if (answer !== 'ok: {"mode":"inline"}')
    failures.push(`the card's fullscreen request was answered ${answer}, expected inline`)
  return expectLive(page, "pane", "fullscreen", failures)
}

/**
 * The test server's chart page, through the real proxy and the host's own
 * document builder. The page is the host: it answers `ui/initialize` and then
 * sends the tool result. The chart goes live and draws that result.
 */
async function chartLive(page, layout, shots) {
  const failures = []
  const dev = await page.evaluate(async () => {
    const answer = await fetch("/src/desktop/widgets/app/model/csp.ts")
    const type = answer.headers.get("content-type") ?? ""
    return answer.ok && type.includes("javascript")
  })
  if (!dev) throw new CannotRun("chart needs the dev server's modules")
  const ready = await page.evaluate(async (html) => {
    const csp = await import("/src/desktop/widgets/app/model/csp.ts")
    const { sandboxMethods: methods } =
      await import("/src/desktop/widgets/app/model/sandbox-methods.ts")
    const origin = await import("/src/desktop/widgets/app/adapters/dom/sandbox-origin.ts")
    const { deadlines } = await import("/src/desktop/widgets/app/application/bridge.ts")
    const sandbox = origin.pageSandbox(document)
    if (!sandbox) return { sandbox: false }
    const applied = csp.appliedCsp({})
    const proxy = document.createElement("iframe")
    proxy.id = "chart-probe"
    proxy.setAttribute("sandbox", origin.proxyFrameSandbox)
    proxy.title = "Chart probe"
    proxy.style.cssText =
      "position:fixed;z-index:9999;left:24px;top:24px;width:420px;height:140px;background:canvas;border:0"
    proxy.src = sandbox.url
    const done = { initialized: false }
    addEventListener("message", (event) => {
      if (event.source !== proxy.contentWindow) return
      const message = event.data
      if (!message || message.jsonrpc !== "2.0") return
      if (message.method === methods.proxyReady) {
        proxy.contentWindow.postMessage(
          {
            jsonrpc: "2.0",
            method: methods.resourceReady,
            params: {
              html: csp.appDocument(html, applied),
              policy: csp.cspPolicy(applied),
              checkWithin: deadlines.initialize,
            },
          },
          sandbox.origin,
        )
        return
      }
      if (message.method === "ui/initialize") {
        proxy.contentWindow.postMessage(
          {
            jsonrpc: "2.0",
            id: message.id,
            result: {
              protocolVersion: "2026-01-26",
              hostInfo: { name: "nessa-test", version: "0" },
              hostCapabilities: {},
              hostContext: { displayMode: "inline" },
            },
          },
          sandbox.origin,
        )
        return
      }
      if (message.method === "ui/notifications/initialized") {
        done.initialized = true
        proxy.contentWindow.postMessage(
          {
            jsonrpc: "2.0",
            method: "ui/notifications/tool-result",
            params: {
              content: [{ type: "text", text: "Chart of two rows." }],
              structuredContent: {
                series: [
                  { name: "alpha", value: 10 },
                  { name: "beta", value: 20 },
                ],
              },
            },
          },
          sandbox.origin,
        )
      }
    })
    document.body.append(proxy)
    const started = performance.now()
    while (!done.initialized && performance.now() - started < 10_000)
      await new Promise((resolve) => setTimeout(resolve, 50))
    return { sandbox: true, initialized: done.initialized }
  }, chartHtml)
  if (!ready.sandbox) throw new CannotRun("the page names no sandbox it may use")
  if (!ready.initialized)
    failures.push("the chart never said ui/notifications/initialized")
  const probe = await page.$("#chart-probe")
  const proxy = await probe?.contentFrame()
  const app = proxy?.childFrames()[0]
  if (!app) {
    failures.push("the chart's document is not in the proxy")
    return { failures }
  }
  await app
    .waitForFunction(
      (series) => document.getElementById("chart")?.textContent === series,
      chartSeries,
      { timeout: 5_000 },
    )
    .catch(() => {})
  const said = await app.evaluate(() => ({
    state: document.body.getAttribute("data-chart-state"),
    chart: document.getElementById("chart")?.textContent ?? null,
    heading: document.querySelector("h1")?.textContent ?? null,
  }))
  if (said.state !== "live") failures.push(`the chart is ${said.state}, not live`)
  if (said.chart !== chartSeries)
    failures.push(`the chart shows ${JSON.stringify(said.chart)}, not the tool result`)
  if (said.heading !== "Chart (nessa-test)")
    failures.push(`the chart's document is not the server's: ${said.heading}`)
  if (shots && probe) {
    mkdirSync(shots, { recursive: true })
    const engine = page.context().browser().browserType().name()
    await probe.screenshot({
      path: join(shots, `chart-${engine}-${layout}.png`),
    })
  }
  return { said, failures }
}

const checks = {
  inline: async (page) => {
    const failures = []
    const { app, box, said } = await expectLive(page, "inline", "inline", failures)
    // The host context it was told: the page's theme and its tokens.
    const told = JSON.parse(said.context ?? "{}")
    const theme = await page.evaluate(() =>
      document.documentElement.classList.contains("dark") ? "dark" : "light",
    )
    if (told.theme !== theme)
      failures.push(`the app was told theme ${told.theme}, not ${theme}`)
    if (!told.styles?.variables?.["--color-text-primary"])
      failures.push(`the app was told no text colour: ${JSON.stringify(told.styles)}`)
    const height = await app.evaluate(() => document.documentElement.scrollHeight)
    await until(
      page,
      ([frame, want]) =>
        Math.abs(document.querySelector(frame).getBoundingClientRect().height - want) <=
        1,
      [selectorFor.appFrameIn("inline"), height],
    ).catch(() => {})
    const drawn = await rect(await page.$(selectorFor.appFrameIn("inline")))
    if (Math.abs(drawn.h - height) > 1)
      failures.push(`the card's frame is ${drawn.h}px tall; the app said ${height}px`)
    return { frame: box, drawn, appHeight: height, told: told.styles, failures }
  },

  pane: async (page) => {
    const failures = []
    const { box } = await openPane(page, failures)
    const card = await appState((await appFrame(page, "inline")).app, "live")
    if (card.mode !== "inline") failures.push(`the card's app is told ${card.mode} after`)
    const measured = await fills(
      page,
      `${css.widgetPane} ${css.widgetBody}`,
      `${css.widgetPane} ${selectorFor.appFrameIn("pane")}`,
    )
    if (!measured.fit)
      failures.push(
        `the pane's frame does not fill its body: ${JSON.stringify(measured)}`,
      )
    return { frame: box, measured, failures }
  },

  window: async (page) => {
    const failures = []
    await openPane(page, failures)
    await page.locator(css.widgetPane).first().hover()
    await page
      .locator(css.widgetPane)
      .first()
      .getByRole("button", { name: names.openInWindow })
      .first()
      .click()
    await contentIs(page, content.widget)
    const { box } = await expectLive(page, "window", "fullscreen", failures)
    const measured = await fills(
      page,
      `${css.widgetWindow} ${css.widgetBody}`,
      selectorFor.appFrameIn("window"),
    )
    if (!measured.fit)
      failures.push(
        `the window's frame does not fill its body: ${JSON.stringify(measured)}`,
      )
    return { frame: box, measured, failures }
  },

  "tools-call": async (page) => {
    const failures = []
    const { app } = await appFrame(page, "inline")
    await appState(app, "live")
    await app.click(selectorFor.fixtureControl("call-allowed"))
    const allowed = await output(app, selectorFor.fixtureOutput("call"))
    if (
      allowed !==
      'ok: {"content":[{"type":"text","text":"Refreshed"}],"structuredContent":{"refreshed":{"times":1}}}'
    )
      failures.push(`the allowed tool was answered ${allowed}`)
    await app.click(selectorFor.fixtureControl("call-hidden"))
    const hidden = await output(app, selectorFor.fixtureOutput("call"), allowed)
    if (hidden !== `error: ${names.hiddenToolRefused}`)
      failures.push(`the hidden tool was answered ${hidden}`)
    return { allowed, hidden, failures }
  },

  message: async (page) => {
    const failures = []
    const { app } = await appFrame(page, "inline")
    await appState(app, "live")
    const authors = page.locator(css.messageAuthor)
    const before = await authors.count()
    // The person's own messages in this session carry no label.
    if (before !== 0) failures.push(`${before} labels before the app wrote anything`)
    await app.click(selectorFor.fixtureControl("message"))
    const sent = await output(app, selectorFor.fixtureOutput("message"))
    if (sent !== "ok: {}") failures.push(`the message was answered ${sent}`)
    await authors
      .nth(before)
      .waitFor({ timeout: 5000 })
      .catch(() => {})
    const count = await authors.count()
    if (count !== before + 1) {
      failures.push(`${count - before} app labels appeared, not 1`)
      return { sent, failures }
    }
    const author = authors.last()
    const label = (await author.textContent().catch(() => null)) ?? ""
    const expected = names.sentBy(names.fixtureTool, names.fixtureServer)
    if (label !== expected) failures.push(`the label says "${label}", not "${expected}"`)
    // Whole on the page, wrapped where it must be: no title repeats it.
    const title = await author.getAttribute("title").catch(() => null)
    if (title !== null) failures.push(`the label has a title, "${title}"`)
    const writer = await author.getAttribute("data-message-app").catch(() => null)
    if (writer !== `${names.fixtureServer}/${names.fixtureTool}`)
      failures.push(`the label's data-message-app is "${writer}"`)
    // Each name isolated from the words around it (#390, E1-8).
    const isolated = await author
      .evaluate((e) => [...e.querySelectorAll("bdi")].map((name) => name.textContent))
      .catch(() => [])
    if (
      JSON.stringify(isolated) !==
      JSON.stringify([names.fixtureTool, names.fixtureServer])
    )
      failures.push(`the label isolates ${JSON.stringify(isolated)}`)
    const wraps = await author
      .evaluate((e) => {
        const style = getComputedStyle(e)
        return style.whiteSpace !== "nowrap" && style.textOverflow !== "ellipsis"
      })
      .catch(() => false)
    if (!wraps) failures.push("the label is cut to one line, not wrapped")
    // The label sits over its own message: the person's, with the app's words.
    const message = author.locator("xpath=..")
    const bubble = message.locator(css.bubble)
    const said = (await bubble.textContent().catch(() => null)) ?? ""
    if (said !== names.fixtureMessage) failures.push(`the bubble says "${said}"`)
    const role = await message.getAttribute("data-role").catch(() => null)
    if (role !== "user") failures.push(`the labelled message is the ${role}'s`)
    const labelRect = await rect(author)
    const bubbleRect = await rect(bubble)
    const column = await rect(message)
    const geometry = { label: labelRect, bubble: bubbleRect, column }
    if (labelRect.y + labelRect.h > bubbleRect.y + 0.5)
      failures.push("the label is not above the bubble")
    const edge = Math.abs(labelRect.x + labelRect.w - (bubbleRect.x + bubbleRect.w))
    geometry.rightEdgeApart = edge
    if (edge > 6) failures.push(`the label is ${edge}px off the bubble's right edge`)
    if (
      labelRect.x < column.x - 0.5 ||
      labelRect.x + labelRect.w > column.x + column.w + 0.5
    )
      failures.push("the label leaves the column")
    // The sample's agent is replying now: another is refused, and adds nothing.
    await app.click(selectorFor.fixtureControl("message"))
    const busy = await output(app, selectorFor.fixtureOutput("message"), sent)
    if (busy !== 'ok: {"isError":true}')
      failures.push(`a message while the reply runs was answered ${busy}`)
    if ((await authors.count()) !== before + 1)
      failures.push("a refused message appeared in the transcript")
    await app.click(selectorFor.fixtureControl("context"))
    const context = await output(app, selectorFor.fixtureOutput("context"))
    // The sample has no model to give a context to, and says so.
    if (context !== `error: ${names.noModelForContext}`)
      failures.push(`the context was answered ${context}`)
    return { sent, label, said, busy, context, geometry, failures }
  },

  csp: async (page) => {
    const failures = []
    const { app } = await appFrame(page, "inline")
    await appState(app, "live")
    await app.click(selectorFor.fixtureControl("fetch"))
    const fetched = await output(app, selectorFor.fixtureOutput("fetch"), "fetching")
    if (fetched !== "blocked") failures.push(`the undeclared fetch was ${fetched}`)
    const notice = page.locator(css.appNotice, { hasText: names.blockedNotice })
    await notice
      .first()
      .waitFor({ timeout: 5000 })
      .catch(() => {})
    if (!(await notice.count()))
      failures.push(
        `no notice "${names.blockedNotice}"; notices: ${JSON.stringify(await page.locator(css.appNotice).allInnerTexts())}`,
      )
    return { fetched, failures }
  },

  isolation: async (page) => {
    const failures = []
    const { proxy, app } = await appFrame(page, "inline")
    await appState(app, "live")
    const inside = await app.evaluate(() => {
      const tries = (run) => {
        try {
          run()
          return "allowed"
        } catch {
          return "refused"
        }
      }
      return {
        origin: window.origin,
        parentDocument: tries(() => window.parent.document.body),
        topDocument: tries(() => window.top.document.body),
        localStorage: tries(() => window.localStorage.getItem("x")),
        // An opaque origin has no cookie jar: reading one throws (Chromium)
        // or a cookie written does not stick (WebKit).
        cookie: tries(() => {
          document.cookie = "mcp-apps-probe=1"
          if (!document.cookie.includes("mcp-apps-probe")) throw new Error("none")
        }),
      }
    })
    const pageOrigin = await page.evaluate(() => window.origin)
    const proxyOrigin = await proxy.evaluate(() => window.origin)
    if (inside.origin !== "null")
      failures.push(`the app's origin is ${inside.origin}, not opaque`)
    for (const reach of ["parentDocument", "topDocument", "localStorage", "cookie"])
      if (inside[reach] !== "refused")
        failures.push(`the app's ${reach} was ${inside[reach]}`)
    if (proxyOrigin === pageOrigin || proxyOrigin === "null")
      failures.push(
        `the proxy's origin ${proxyOrigin} is not another origin than ${pageOrigin}`,
      )
    return { inside, pageOrigin, proxyOrigin, failures }
  },

  ...Object.fromEntries(
    // The navigation is refused, and either the frame never left the app's
    // document (WebKit: the app stays, live) or it did, and the host says so
    // — how soon: the reporter's word on \`pagehide\`, well before the
    // check's deadline; a rewrite erases the reporter, so the departure is
    // its load's check going unanswered, past the host's initialize
    // deadline.
    Object.entries({ navigate: 5000, refresh: 5000, rewrite: 20_000 }).map(
      ([control, within]) => [
        `escape-${control}`,
        async (page) => {
          const failures = []
          const leaked = []
          page.on("request", (request) => {
            if (new URL(request.url()).hostname === "example.com")
              leaked.push(request.url())
          })
          const { app } = await appFrame(page, "inline")
          await appState(app, "live")
          // This document, marked: a document after it cannot carry the mark
          // (a rewrite keeps the window, so the fixture's controls are asked
          // for too).
          const mark = await app.evaluate(
            () => (window.mcpAppsMark = String(Math.random())),
          )
          await app.click(selectorFor.fixtureControl(control))
          const line = page.locator(css.appView, { hasText: names.appLoadLine })
          await line
            .first()
            .waitFor({ timeout: within })
            .catch(() => {})
          const departed = (await line.count()) > 0
          const stayed =
            !departed &&
            !app.isDetached() &&
            (await app
              .evaluate(
                // The mark survives a rewrite (the window stays): the fixture's
                // own controls do not.
                ([marked, control]) =>
                  window.mcpAppsMark === marked &&
                  location.href === "about:srcdoc" &&
                  document.querySelector(control) !== null,
                [mark, selectorFor.fixtureControl(control)],
              )
              .catch(() => false)) &&
            (await page.locator(css.appView).first().getAttribute("data-app-view")) ===
              "live"
          if (!departed && !stayed)
            failures.push(
              `within ${within} ms of ${control}, the host does not say "${names.appLoadLine}", and the app's document is not the one it was`,
            )
          if (departed && (await page.$(selectorFor.appFrameIn("inline"))))
            failures.push(`after ${control}, the app's frame is still on the page`)
          if (leaked.length > 0)
            failures.push(`requests reached example.com: ${leaked.join(", ")}`)
          return { outcome: departed ? "departed" : "stayed", leaked, failures }
        },
      ],
    ),
  ),

  departures: (page) => departuresOn(page, departureScenarios),

  "departures-back": (page) => departuresOn(page, backScenarios),

  chart: (page, layout, shots) => chartLive(page, layout, shots),

  forge: async (page) => {
    const failures = []
    const { app } = await appFrame(page, "inline")
    await appState(app, "live")
    // Speaking as the proxy, and putting words in the host's chrome.
    await app.click(selectorFor.fixtureControl("forge"))
    await page.waitForTimeout(500)
    const view = await page.locator(css.appView).first().getAttribute("data-app-view")
    if (view !== "live")
      failures.push(`after the forged proxy messages the app is ${view}`)
    const said = await appState(app, "live", 1000)
    if (said.state !== "live") failures.push(`the app is ${said.state} after forging`)
    const notices = await page.locator(css.appNotice).allInnerTexts()
    for (const notice of notices)
      if (!names.appNotices.some((line) => line.test(notice)))
        failures.push(`a notice that is not the host's own reached its chrome: ${notice}`)
    return { notices, failures }
  },

  teardown: async (page) => {
    const failures = []
    // By its pane's close: the frames go with the place.
    const { proxy, app } = await openPane(page, failures)
    const pane = page.locator(css.widgetPane).first()
    await pane.hover()
    await pane.getByRole("button", { name: names.closePane }).first().click()
    await settled(page)
    await until(
      page,
      (frame) => !document.querySelector(frame),
      selectorFor.appFrameIn("pane"),
    ).catch(() => {})
    if (await page.$(selectorFor.appFrameIn("pane")))
      failures.push("the pane's frame is still on the page after its close")
    if (!proxy.isDetached() || !app.isDetached())
      failures.push("the pane's proxy or app document outlived its close")
    // By its own request: teardown is sent, answered, and the pane closes.
    const second = await openPane(page, failures)
    const panes = await paneCount(page)
    // The fixture answers its teardown 400ms after it is asked: until then
    // the pane stays, and the app is still on the page saying so.
    await second.app.click(selectorFor.fixtureControl("close"))
    const said = await appState(second.app, "tearing-down", 2000).catch(() => ({
      state: "detached",
    }))
    const waiting = await paneCount(page)
    await paneCountIs(page, panes - 1)
    if (said.state !== "tearing-down")
      failures.push(
        `the app asking to go was not sent ui/resource-teardown (it says ${said.state})`,
      )
    if (waiting !== panes)
      failures.push("the app's pane closed before the app answered its teardown")
    if ((await paneCount(page)) !== panes - 1)
      failures.push("the app's pane did not close after its teardown")
    if (!second.app.isDetached())
      failures.push("the app's document outlived its teardown")
    return { said: said.state, failures }
  },
}

await main(
  meta,
  async ({ options, rep, url, mode }) => {
    const only = options.only ? options.list(options.only) : Object.keys(checks)
    for (const name of only)
      if (!Object.hasOwn(checks, name)) throw new CannotRun(`no check named ${name}`)
    await withEngines(options, rep, async (engine, browser) => {
      for (const layout of options.layouts)
        for (const name of only) {
          if (
            recordIfLeftOut(rep, mode, name, devServerOnlySteps["mcp-apps"], {
              engine,
              layout,
            })
          )
            continue
          let opened
          await attempt(rep, { engine, layout, name }, async () => {
            opened = await onSample(browser, { url, layout })
            const result = await checks[name](opened.page, layout, options.shots)
            return result
          }).finally(() => opened?.close())
        }
    })
  },
  // A preview this check would start has no sandbox meta: the plugin publishes
  // the proxy only from configureServer. Refuse before that build. An explicit
  // --url may be a dev server, which does have the proxy.
  async (options) => {
    if (options.mode === "prod" && !options.url)
      throw new CannotRun(
        "MCP Apps fixtures need the dev server's sandbox proxy, which starts in configureServer and is absent from vite preview. Under --mode prod this check is not run.",
      )
    return target(options)
  },
)
