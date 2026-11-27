// @vitest-environment jsdom
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { NessaRpcError, type CheckedAppPackage, type InstalledApp } from "@nessa/client"
import type { InstalledAppsGateway } from "../adapters/installed-apps-gateway"
import { InstalledAppsProvider, InstalledAppsTab } from "./installed-apps-tab"

const host = vi.hoisted(() => ({
  confirmAppInstall: vi.fn(async (_review: unknown) => false),
  openInstalledApp: vi.fn(async (..._args: unknown[]) => undefined),
  placeInstalledApp: vi.fn(async (..._args: unknown[]) => undefined),
  removeInstalledApp: vi.fn(async (_appId: string) => undefined),
  keepOnlyInstalledApps: vi.fn(async (_appIds: readonly string[]) => [] as string[]),
  stopInstalledApp: vi.fn(async (_appId: string) => undefined),
  runningInstalledApps: vi.fn(async () => [] as string[]),
  onInstalledAppsChanged: vi.fn(async (_handler: () => void) => () => undefined),
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
})

afterEach(() => {
  act(() => root.unmount())
  container.remove()
})

function checked(appId: string, packageVersion: string): CheckedAppPackage {
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
    },
  }
}

/** A gateway that keeps what it installs, as the real one does. */
function keeping() {
  const installed = new Map<string, InstalledApp>()
  const gateway = {
    starter: vi.fn(async (appId: string, version: string) => {
      if (appId === "Nope")
        throw new NessaRpcError("app_package_invalid", "invalid", undefined)
      return checked(appId, version)
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
    read: vi.fn(async (appId: string) => checked(appId, "1.0.0")),
    uninstall: vi.fn(async (appId: string) => {
      installed.delete(appId)
    }),
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
  expect(host.stopInstalledApp.mock.invocationCallOrder[0]).toBeLessThan(
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
