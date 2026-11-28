// @vitest-environment jsdom
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import {
  NessaRpcError,
  type AppCapability,
  type AppServerRuntime,
  type CheckedAppPackage,
  type InstalledApp,
} from "@nessa/client"
import type { InstalledAppsGateway } from "../adapters/installed-apps-gateway"
import { InstalledAppsProvider, InstalledAppsTab } from "./installed-apps-tab"

const host = vi.hoisted(() => ({
  confirmAppInstall: vi.fn(async (..._args: unknown[]) => false),
  openInstalledApp: vi.fn(async (..._args: unknown[]) => undefined),
  placeInstalledApp: vi.fn(async (..._args: unknown[]) => undefined),
  removeInstalledApp: vi.fn(async (_appId: string) => undefined),
  keepOnlyInstalledApps: vi.fn(async (_appIds: readonly string[]) => [] as string[]),
  stopInstalledApp: vi.fn(async (_appId: string) => undefined),
  runningInstalledApps: vi.fn(async () => [] as string[]),
  onInstalledAppsChanged: vi.fn(async (_handler: () => void) => () => undefined),
  handInstalledAppCredential: vi.fn(async (..._args: unknown[]) => undefined),
  installedAppHasCredential: vi.fn(async (_appId: string) => false),
}))
vi.mock("../../../host", () => host)

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
  host.confirmAppInstall.mockReset().mockResolvedValue(false)
  host.openInstalledApp.mockReset().mockResolvedValue(undefined)
  host.placeInstalledApp.mockReset().mockResolvedValue(undefined)
  host.removeInstalledApp.mockReset().mockResolvedValue(undefined)
  host.keepOnlyInstalledApps.mockReset().mockResolvedValue([])
  host.stopInstalledApp.mockReset().mockResolvedValue(undefined)
  host.runningInstalledApps.mockReset().mockResolvedValue([])
  host.handInstalledAppCredential.mockReset().mockResolvedValue(undefined)
  host.installedAppHasCredential.mockReset().mockResolvedValue(false)
})

afterEach(() => {
  act(() => root.unmount())
  container.remove()
})

function checked(
  appId: string,
  packageVersion: string,
  server?: AppServerRuntime,
): CheckedAppPackage {
  return {
    package: {
      manifest: btoa(`${appId}@${packageVersion}`),
      assets: [{ path: "index.html", bytes: btoa("<h1>hi</h1>") }],
    },
    review: {
      appId,
      packageVersion,
      entrypoint: "index.html",
      assetCount: 1,
      digest: "a".repeat(64),
      requestedCapabilities: server ? ["list", "read", "create", "send"] : [],
      ...(server ? { server: { entry: "server/main.mjs", runtime: server } } : {}),
    },
  }
}

/** A gateway that keeps what it installs, as the real one does. */
function keeping() {
  const installed = new Map<string, InstalledApp>()
  const servers = new Map<string, AppServerRuntime>()
  const gateway = {
    starter: vi.fn(async (appId: string, version: string, server?: AppServerRuntime) => {
      if (appId === "Nope")
        throw new NessaRpcError("app_package_invalid", "invalid", undefined)
      if (server) servers.set(appId, server)
      return checked(appId, version, server)
    }),
    review: vi.fn(async () => checked("x", "1").review),
    install: vi.fn(async (pkg: CheckedAppPackage["package"], digest: string) => {
      void pkg
      expect(digest).toBe("a".repeat(64))
      const app = { appId: "notes", packageVersion: "1.0.0" }
      installed.set(app.appId, app)
      return app
    }),
    installed: vi.fn(async () => [...installed.values()]),
    read: vi.fn(async (appId: string) => checked(appId, "1.0.0", servers.get(appId))),
    uninstall: vi.fn(async (appId: string) => {
      installed.delete(appId)
    }),
    issueCredential: vi.fn(async (appId: string, requestId: string) => ({
      requestId,
      appId,
      credentialId: `credential-${appId}`,
      credential: `secret-for-${appId}`,
      capabilities: ["list", "read", "create", "send"] as AppCapability[],
      expiresAt: 1,
    })),
  } satisfies InstalledAppsGateway
  return gateway
}

async function settle() {
  for (let i = 0; i < 5; i++) await act(async () => await Promise.resolve())
}

function draw(gateway: InstalledAppsGateway) {
  act(() =>
    root.render(
      <InstalledAppsProvider gateway={gateway} requestId={() => "req"}>
        <InstalledAppsTab />
      </InstalledAppsProvider>,
    ),
  )
}

function type(selector: string, value: string) {
  const input = container.querySelector<HTMLInputElement>(selector)
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set
  if (!input || !setter) throw new Error(`no field ${selector}`)
  act(() => {
    setter.call(input, value)
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
}

async function press(selector: string) {
  const button = container.querySelector<HTMLButtonElement>(selector)
  if (!button) throw new Error(`no button ${selector}`)
  act(() => button.click())
  await settle()
}

it("installs nothing when the person cancels the system's dialog", async () => {
  const gateway = keeping()
  draw(gateway)
  await settle()
  type("[data-create-app-id]", "notes")
  await press("[data-create-app]")
  expect(host.confirmAppInstall).toHaveBeenCalledWith(
    expect.objectContaining({ appId: "notes", digest: "a".repeat(64) }),
    checked("notes", "1.0.0").package,
  )
  expect(gateway.install).not.toHaveBeenCalled()
  expect(host.placeInstalledApp).not.toHaveBeenCalled()
  expect(container.querySelector("[data-installed-app]")).toBeNull()
  // Settings draws no app: the dialog is the review.
  expect(container.querySelector("iframe")).toBeNull()
})

it("installs the reviewed digest on Yes, then opens, stops and uninstalls it", async () => {
  const gateway = keeping()
  host.confirmAppInstall.mockResolvedValue(true)
  draw(gateway)
  await settle()
  type("[data-create-app-id]", "notes")
  await press("[data-create-app]")
  expect(gateway.install).toHaveBeenCalledTimes(1)
  // An open app of that name stops once its install is done, before its
  // credential is issued again.
  expect(host.stopInstalledApp).toHaveBeenCalledWith("notes")
  expect(gateway.install.mock.invocationCallOrder[0]).toBeLessThan(
    host.stopInstalledApp.mock.invocationCallOrder[0] ?? 0,
  )
  // Installed, it is an app of its own: the host places its launcher.
  expect(host.placeInstalledApp).toHaveBeenCalledWith(
    "notes",
    checked("notes", "1.0.0").package,
    "a".repeat(64),
  )
  expect(container.querySelector('[data-installed-app="notes"]')).not.toBeNull()
  expect(container.querySelector<HTMLInputElement>("[data-create-app-id]")?.value).toBe(
    "",
  )

  await press('[data-installed-app-open="notes"]')
  expect(host.openInstalledApp).toHaveBeenCalledWith(
    "notes",
    checked("notes", "1.0.0").package,
    "a".repeat(64),
  )

  await press('[data-installed-app-uninstall="notes"]')
  expect(host.stopInstalledApp).toHaveBeenCalledWith("notes")
  expect(gateway.uninstall).toHaveBeenCalledWith("notes", "req")
  expect(host.removeInstalledApp).toHaveBeenCalledWith("notes")
  expect(host.stopInstalledApp.mock.invocationCallOrder.at(-1)).toBeLessThan(
    host.removeInstalledApp.mock.invocationCallOrder[0] ?? 0,
  )
  expect(container.querySelector('[data-installed-app="notes"]')).toBeNull()
})

it("keeps only the launchers of apps the gateway lists after each read", async () => {
  const gateway = keeping()
  await gateway.install(checked("notes", "1.0.0").package, "a".repeat(64))
  draw(gateway)
  await settle()
  expect(host.keepOnlyInstalledApps).toHaveBeenLastCalledWith(["notes"])

  await press('[data-installed-app-uninstall="notes"]')
  expect(host.keepOnlyInstalledApps).toHaveBeenLastCalledWith([])

  // While a read is out nothing can change: no row and no Create to press.
  let answer: (apps: InstalledApp[]) => void = () => undefined
  gateway.installed.mockImplementationOnce(
    () => new Promise<InstalledApp[]>((resolve) => (answer = resolve)),
  )
  act(() => root.unmount())
  root = createRoot(container)
  draw(gateway)
  await settle()
  expect(
    container.querySelector<HTMLInputElement>("[data-create-app-id]")?.disabled,
  ).toBe(true)
  answer([])
  await settle()
  expect(
    container.querySelector<HTMLInputElement>("[data-create-app-id]")?.disabled,
  ).toBe(false)
})

it("says why the gateway refused a new app", async () => {
  const gateway = keeping()
  draw(gateway)
  await settle()
  type("[data-create-app-id]", "Nope")
  await press("[data-create-app]")
  expect(
    container.querySelector('[data-installed-apps-said="problem"]')?.textContent,
  ).toContain("isn’t a valid app id")
})

async function pickKind(value: string) {
  const trigger = container.querySelector<HTMLElement>(
    "[data-choice-trigger][data-create-app-kind], [data-create-app-kind] [data-choice-trigger]",
  )
  if (!trigger) throw new Error("no kind menu")
  await act(async () => {
    trigger.focus()
    trigger.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
    )
  })
  const option = document.querySelector<HTMLElement>(
    `[role="menuitemradio"][data-value="${value}"]`,
  )
  if (!option) throw new Error(`no kind ${value}`)
  await act(async () => option.click())
}

it("hands a new app with a server its credential, and only that app", async () => {
  const gateway = keeping()
  host.confirmAppInstall.mockResolvedValue(true)
  draw(gateway)
  await settle()
  await pickKind("bun")
  type("[data-create-app-id]", "notes")
  await press("[data-create-app]")
  expect(gateway.starter).toHaveBeenCalledWith("notes", "1.0.0", "bun")
  expect(host.confirmAppInstall).toHaveBeenCalledWith(
    expect.objectContaining({ server: { entry: "server/main.mjs", runtime: "bun" } }),
    expect.anything(),
  )
  // Placed first, so the credential has its launcher to go in.
  expect(gateway.issueCredential).toHaveBeenCalledWith("notes", "req")
  expect(host.handInstalledAppCredential).toHaveBeenCalledWith(
    "notes",
    "secret-for-notes",
    1,
  )
  expect(host.placeInstalledApp.mock.invocationCallOrder[0]).toBeLessThan(
    host.handInstalledAppCredential.mock.invocationCallOrder[0] ?? 0,
  )

  // Its launcher lost its credential: Open issues one before it runs.
  await press('[data-installed-app-open="notes"]')
  expect(gateway.issueCredential).toHaveBeenCalledTimes(2)
  expect(host.handInstalledAppCredential.mock.invocationCallOrder[1]).toBeLessThan(
    host.openInstalledApp.mock.invocationCallOrder[0] ?? 0,
  )
  // Still holding one, it is opened as it is.
  host.installedAppHasCredential.mockResolvedValue(true)
  await press('[data-installed-app-open="notes"]')
  expect(gateway.issueCredential).toHaveBeenCalledTimes(2)
  expect(host.openInstalledApp).toHaveBeenCalledTimes(2)
})

it("issues no credential under a server the host says is running", async () => {
  const gateway = keeping()
  host.confirmAppInstall.mockResolvedValue(true)
  draw(gateway)
  await settle()
  await pickKind("node")
  type("[data-create-app-id]", "notes")
  await press("[data-create-app]")
  expect(gateway.issueCredential).toHaveBeenCalledTimes(1)
  // Opened from the Dock a moment ago: the list has not caught up, the host
  // has. Issuing would end the credential that server signed in with.
  host.runningInstalledApps.mockResolvedValue(["notes"])
  await press('[data-installed-app-open="notes"]')
  expect(gateway.issueCredential).toHaveBeenCalledTimes(1)
  expect(host.openInstalledApp).toHaveBeenCalledTimes(1)
})

it("starts again an app opened while its credential was replaced", async () => {
  const gateway = keeping()
  host.confirmAppInstall.mockResolvedValue(true)
  draw(gateway)
  await settle()
  await pickKind("node")
  type("[data-create-app-id]", "notes")
  await press("[data-create-app]")
  host.stopInstalledApp.mockClear()
  // Not running when asked; opened from the Dock while the new one was
  // issued, so it started with the one issuing ended.
  host.runningInstalledApps.mockResolvedValueOnce([]).mockResolvedValueOnce(["notes"])
  await press('[data-installed-app-open="notes"]')
  expect(gateway.issueCredential).toHaveBeenCalledTimes(2)
  expect(host.stopInstalledApp).toHaveBeenCalledWith("notes")
  expect(host.handInstalledAppCredential.mock.invocationCallOrder[1]).toBeLessThan(
    host.stopInstalledApp.mock.invocationCallOrder[0] ?? 0,
  )
  expect(host.stopInstalledApp.mock.invocationCallOrder[0]).toBeLessThan(
    host.openInstalledApp.mock.invocationCallOrder[0] ?? 0,
  )
})

it("hands a page no credential", async () => {
  const gateway = keeping()
  host.confirmAppInstall.mockResolvedValue(true)
  draw(gateway)
  await settle()
  type("[data-create-app-id]", "notes")
  await press("[data-create-app]")
  expect(gateway.starter).toHaveBeenCalledWith("notes", "1.0.0", undefined)
  await press('[data-installed-app-open="notes"]')
  expect(gateway.issueCredential).not.toHaveBeenCalled()
  expect(host.handInstalledAppCredential).not.toHaveBeenCalled()
  expect(host.installedAppHasCredential).not.toHaveBeenCalled()
})
