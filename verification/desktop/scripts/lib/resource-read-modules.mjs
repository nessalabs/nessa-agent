/** Opt-in Vite response decoration, installed before the gateway page loads. */
import { randomUUID } from "node:crypto"
import { readFile } from "node:fs/promises"
import { resourceEndpoint, resourceReadEvidence } from "./resource-read-evidence.mjs"

const factories = new Map([
  ["/src/desktop/widgets/app/application/bridge.ts", ["createAppBridge", "bridge"]],
  [
    "/packages/nessa-client/src/presentation/mcp-apps-api.ts",
    ["createMcpAppsApi", "api"],
  ],
  [
    "/packages/nessa-client/src/transport/mcp-resource-fetch.ts",
    ["fetchMcpResource", "transport"],
  ],
])

export function decorateResourceModule(source, name, kind, moduleUrl) {
  const declaration = `export function ${name}(`
  if (source.split(declaration).length !== 2)
    throw new Error("unsupported resource factory export")
  return `${source.replace(declaration, `function ${name}(`)}\nimport { observation as __nessaObservation } from ${JSON.stringify(moduleUrl)};\nconst __nessaObservedFactory = __nessaObservation.${kind}(${name});\nexport { __nessaObservedFactory as ${name} };\n`
}

export async function installResourceObservation(page) {
  const generation = randomUUID()
  const binding = `__nessaResource${randomUUID().replaceAll("-", "")}`
  const moduleUrl = `/__nessa_verification_resource_${generation}.mjs`
  const collector = resourceReadEvidence(generation)
  const source = await readFile(
    new URL("./resource-read-observer.mjs", import.meta.url),
    "utf8",
  )
  const module = `${source}\nconst observation = createResourceObserver((message) => globalThis[${JSON.stringify(binding)}](message), ${JSON.stringify(generation)});\nexport { observation };\n`
  let navigated = false
  let finalizing
  await page.exposeBinding(binding, (origin, message) => {
    if (origin.page !== page || origin.frame !== page.mainFrame()) {
      collector.invalidate()
      return
    }
    collector.receive(message)
  })
  page.on("framenavigated", (frame) => {
    if (frame !== page.mainFrame()) return
    if (navigated) collector.invalidate()
    navigated = true
  })
  page.on("request", (request) => {
    try {
      if (request.frame() !== page.mainFrame()) return
      if (finalizing && resourceEndpoint(request)) collector.invalidate()
      collector.observeRequest(request)
    } catch {
      collector.invalidate()
    }
  })
  await page.route(
    new RegExp(
      `(?:${[...factories.keys(), moduleUrl].map((path) => path.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})(?:\\?.*)?$`,
    ),
    async (route) => {
      try {
        const path = new URL(route.request().url()).pathname
        if (path === moduleUrl) {
          await route.fulfill({
            status: 200,
            contentType: "text/javascript",
            body: module,
          })
          return
        }
        const response = await route.fetch()
        if (
          response.status() !== 200 ||
          !/^(?:text|application)\/javascript(?:;|$)/.test(
            response.headers()["content-type"] ?? "",
          )
        )
          throw new Error("unsupported resource module response")
        const [name, kind] = factories.get(path)
        await route.fulfill({
          response,
          body: decorateResourceModule(await response.text(), name, kind, moduleUrl),
        })
      } catch {
        collector.invalidate()
        await route.abort("failed").catch(() => {})
      }
    },
  )
  return {
    collector,
    finalize() {
      if (!finalizing)
        finalizing = page
          .evaluate(async (url) => {
            const { observation } = await import(url)
            await observation.finalize()
          }, moduleUrl)
          .catch(() => collector.invalidate())
      return finalizing
    },
  }
}
