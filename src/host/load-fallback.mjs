/**
 * The document shown until the page's module runs, and what it says when that
 * module never arrives.
 *
 * Vite injects this into `index.html` and `desktop.html`. Until the module
 * runs it says Loading. If the module never arrives, the same calm screen
 * the running page uses replaces it: the shared line, a code, and icon
 * actions. The filled sentence is logged, not shown. The bootstrap is
 * inline: a held-back script request must not be able to take it with it.
 */
import { readFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { flatStartupMark } from "./startup-mark.mjs"

const here = dirname(fileURLToPath(import.meta.url))

const sentences = JSON.parse(readFileSync(resolve(here, "startup-refusals.json"), "utf8"))
const faceTemplate = readFileSync(resolve(here, "startup-face.html"), "utf8").replace(
  "{{MARK}}",
  flatStartupMark(
    readFileSync(resolve(here, "../../src-tauri/icons/nessa-avatar.svg"), "utf8"),
  ),
)
const screenCss = readFileSync(resolve(here, "startup-screen.css"), "utf8")
const actionsSource = readFileSync(resolve(here, "startup-actions.js"), "utf8")

export const LOAD_FALLBACK_WAIT_MS = 15_000

const style = `
    <style>
      [data-nessa-load-fallback] {
        position: fixed;
        inset: 0;
        box-sizing: border-box;
        overflow: hidden;
        color: #ededed;
        background: #121214;
        font:
          14px/1.5 -apple-system,
          BlinkMacSystemFont,
          system-ui,
          sans-serif;
        -webkit-font-smoothing: antialiased;
      }

      /* The same drifting light the window paints behind its glass. */
      [data-nessa-load-fallback]::before,
      [data-nessa-load-fallback]::after {
        content: "";
        position: absolute;
        width: 70vmax;
        height: 70vmax;
        border-radius: 50%;
        filter: blur(80px);
        opacity: 0.55;
        animation: nessa-load-drift 28s ease-in-out infinite alternate;
      }

      [data-nessa-load-fallback]::before {
        left: 10%;
        top: -35%;
        background: radial-gradient(
          closest-side,
          oklch(0.52 0.15 295 / 42%),
          transparent
        );
      }

      [data-nessa-load-fallback]::after {
        right: -20%;
        bottom: -45%;
        background: radial-gradient(closest-side, oklch(0.6 0.13 35 / 22%), transparent);
        animation-duration: 36s;
        animation-direction: alternate-reverse;
      }

      @keyframes nessa-load-drift {
        to {
          transform: translate3d(6vw, 4vh, 0) scale(1.08);
        }
      }

      [data-nessa-load-message] {
        position: fixed;
        z-index: 1;
        box-sizing: border-box;
        max-width: 100vw;
        max-height: 100vh;
        display: grid;
        place-content: center;
        justify-items: center;
        gap: 0.75rem;
        padding: 1.5rem;
        overflow-wrap: anywhere;
        text-align: center;
      }

      /* The panel's webview is a stage larger than its window, pinned to the
         window's bottom right (src/panel/adapters/panel-frame.ts). Until
         main.tsx writes the window's size, stay inside the bottom-right
         320 × 320 that every panel the host opens covers (MIN_PANEL_HEIGHT,
         and narrower than the default minimum width). */
      html[data-nessa-surface="panel"] [data-nessa-load-message] {
        right: 0;
        bottom: 0;
        width: var(--nessa-window-width, 320px);
        height: var(--nessa-window-height, 320px);
      }

      html[data-nessa-surface="setup"] [data-nessa-load-message] {
        inset: 0;
      }

      /* The desktop window has no surface attribute, so the message fills it. */
      html:not([data-nessa-surface]) [data-nessa-load-message] {
        inset: 0;
      }

      [data-nessa-load-mark] {
        width: 56px;
        height: 56px;
        border-radius: 50%;
        box-shadow: 0 0 36px oklch(0.6 0.16 295 / 40%);
        animation: nessa-load-breathe 2.4s ease-in-out infinite;
      }

      @keyframes nessa-load-breathe {
        50% {
          transform: scale(0.88);
          opacity: 0.7;
          box-shadow: 0 0 18px oklch(0.6 0.16 295 / 25%);
        }
      }

      [data-nessa-load-title] {
        color: rgb(237 237 237 / 60%);
        font-size: 0.8125rem;
        letter-spacing: 0.02em;
      }

      @media (prefers-reduced-motion: reduce) {
        [data-nessa-load-fallback]::before,
        [data-nessa-load-fallback]::after,
        [data-nessa-load-mark] {
          animation: none;
        }
      }
    </style>`

const rootMarkup = `<div id="root">
      <main data-nessa-load-fallback>
        <div data-nessa-load-message role="status">
          <img data-nessa-load-mark src="/src-tauri/icons/nessa-avatar.svg" alt="" />
          <span data-nessa-load-title>Loading</span>
        </div>
      </main>
    </div>`

function embedSource(value) {
  return JSON.stringify(value).replaceAll("<", "\\u003c")
}

function bootstrap() {
  const copies = embedSource(sentences)
  const face = embedSource(faceTemplate)
  const actions = actionsSource.replaceAll("</", "<\\/")
  return `<script data-nessa-load-bootstrap>
      ;(() => {
        const copies = ${copies}
        const faceTemplate = ${face}
        const waitMs = ${LOAD_FALLBACK_WAIT_MS}
        ${actions}
        const title = document.querySelector("[data-nessa-load-title]")
        const fill = (key, values) => {
          const text = Object.hasOwn(copies, key) ? copies[key] : ""
          if (typeof text !== "string") return key
          return Object.entries(values).reduce(
            (current, [name, value]) => current.replaceAll("{" + name + "}", value),
            text,
          )
        }
        const page = () => location.href
        const mounted = () => document.documentElement.dataset.nessaMounted === "1"
        const escapeText = (text) =>
          String(text)
            .replaceAll("&", "&amp;")
            .replaceAll("<", "&lt;")
            .replaceAll(">", "&gt;")
        const codeOf = (key) => {
          const table = copies.code
          if (!table || typeof table !== "object" || !Object.hasOwn(table, key)) return key
          const value = table[key]
          return typeof value === "string" && value ? value : key
        }
        const line = () =>
          typeof copies.line === "string" && copies.line ? copies.line : ""
        // The person sees the line and the code. The filled sentence is the log.
        const showFailure = (key, detail) => {
          if (mounted()) return
          if (document.documentElement.dataset.nessaStartupShown === "1") return
          document.documentElement.dataset.nessaStartupShown = "1"
          console.error("[nessa] " + detail)
          const message = document.querySelector("[data-nessa-load-message]")
          if (!message) return
          message.setAttribute("role", "alert")
          message.setAttribute("data-nessa-startup-screen", "")
          message.innerHTML = faceTemplate
            .replaceAll("{{LINE}}", escapeText(line()))
            .replaceAll("{{CODE}}", escapeText(codeOf(key)))
          wireStartupActions(message)
        }
        // The bootstrap is parsed before the app's module tag, and Vite inserts
        // an inline module ahead of it. The app entry is the last module that
        // names a src. Look it up when the failure happens, not while parsing.
        const moduleScript = () => {
          const nodes = document.querySelectorAll("script[type=module][src]")
          return nodes.length ? nodes[nodes.length - 1] : null
        }
        const scriptUrl = (element) => {
          const node = element || moduleScript()
          return node && node.getAttribute ? node.getAttribute("src") || "" : ""
        }
        const showUnserved = (element) => {
          if (mounted()) return
          showFailure(
            "script-unserved",
            fill("script-unserved", { page: page(), script: scriptUrl(element) }),
          )
        }
        const isScript = (target) =>
          !!target && target !== window && String(target.tagName).toUpperCase() === "SCRIPT"
        // A document that already contains the module (tests, a later eval)
        // can fail before this listener's capture path runs.
        moduleScript()?.addEventListener("error", (event) => showUnserved(event.target))
        window.addEventListener(
          "error",
          (event) => {
            if (mounted()) return
            // A module fetch that fails targets the script element and does
            // not bubble. Capture still sees it, including when the tag is
            // inserted after this script runs.
            if (isScript(event.target)) {
              showUnserved(event.target)
              return
            }
            const message = event.message || "the page stopped while starting"
            showFailure("runtime", "Could not load " + page() + ". " + message)
          },
          true,
        )
        window.addEventListener("unhandledrejection", (event) => {
          if (document.documentElement.dataset.nessaMounted === "1") return
          const reason = event.reason
          const message = reason instanceof Error ? reason.message : String(reason ?? "")
          showFailure("runtime", "Could not load " + page() + ". " + message)
        })
        setTimeout(() => {
          if (document.documentElement.dataset.nessaMounted === "1") return
          if (document.documentElement.dataset.nessaModule === "started") return
          if (document.documentElement.dataset.nessaStartupShown === "1") return
          if (!title || title.textContent !== "Loading") return
          showFailure(
            "still-compiling",
            fill("still-compiling", { page: page(), script: scriptUrl() }),
          )
        }, waitMs)
      })()
    </script>`
}

/** Inject the fallback into an app document that does not have one yet. */
export function embedLoadFallback(html) {
  if (html.includes("data-nessa-load-fallback")) return html
  if (!html.includes("</head>")) throw new Error("load fallback found no </head>")
  if (!/<div id="root">\s*<\/div>/.test(html))
    throw new Error("load fallback found no empty #root")
  if (!html.includes('<script type="module"'))
    throw new Error("load fallback found no module script")
  return html
    .replace("</head>", `<style>\n${screenCss}\n</style>\n${style}\n  </head>`)
    .replace(/<div id="root">\s*<\/div>/, rootMarkup)
    .replace('<script type="module"', `${bootstrap()}\n    <script type="module"`)
}
