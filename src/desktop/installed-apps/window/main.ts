/**
 * An installed app's own window (`installed-app.html`): what the host holds
 * for this window (`installed_app_page`, verified bytes only), made into one
 * document and drawn in the sandbox proxy's opaque frame. The page holds no
 * gateway credential and calls nothing else; closing the window, or Stop in
 * Settings, ends the app.
 */
import { host, installedAppPage, sandboxSchemeRegistered } from "../../../host"
import { sandboxFor } from "../../widgets/app/adapters/dom/sandbox-origin"
import { packageDocument } from "../model/app-document"
import { mountStaticApp } from "../adapters/static-frame"

const root = document.getElementById("app")
const say = (words: string) => {
  if (!root) return
  root.textContent = ""
  const line = document.createElement("p")
  line.className = "installed-app-line"
  line.textContent = words
  root.appendChild(line)
  root.dataset.installedApp = "unshown"
}

async function draw(): Promise<void> {
  if (!root) return
  const sandbox = sandboxFor(host.kind, document, sandboxSchemeRegistered())
  if (!sandbox) return say("This app can’t be shown here: there is no sandbox.")
  const page = await installedAppPage()
  document.title = page.appId
  const html = packageDocument(page.entrypoint, page.assets, (text) =>
    new DOMParser().parseFromString(text, "text/html"),
  )
  root.textContent = ""
  root.dataset.installedApp = page.appId
  mountStaticApp(root, sandbox, html, {
    title: page.appId,
    onLeft: () => say(`${page.appId} has stopped.`),
  })
}

draw().catch(() =>
  say("This app couldn’t be opened. Close the window and open it again."),
)
