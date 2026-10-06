import { expect, it } from "vitest"
import { workspaceBackend } from "./workspace-backend"

it("the desktop app reads the host's gateway, whatever the page was opened with (C1)", () => {
  for (const host of ["macos", "linux", "other"] as const) {
    expect(workspaceBackend(host, "")).toBe("host")
    expect(workspaceBackend(host, "?gateway")).toBe("host")
  }
})

it("a browser preview reads the gateway only when its address asks (C2, C3)", () => {
  expect(workspaceBackend("browser", "?gateway")).toBe("browser")
  expect(workspaceBackend("browser", "?theme=dark&gateway=1")).toBe("browser")
  expect(workspaceBackend("browser", "")).toBe("sample")
  expect(workspaceBackend("browser", "?gateways")).toBe("sample")
  expect(workspaceBackend("browser", "?theme=gateway")).toBe("sample")
})

it("a browser page with ?seeded is the seeded workspace, and ?gateway wins (C4)", () => {
  expect(workspaceBackend("browser", "?seeded=590")).toBe("seeded")
  expect(workspaceBackend("browser", "?seeded=590&now=1")).toBe("seeded")
  expect(workspaceBackend("browser", "?gateway&seeded=590")).toBe("browser")
  expect(workspaceBackend("browser", "?seeded=590&gateway=1")).toBe("browser")
  expect(workspaceBackend("browser", "?theme=seeded")).toBe("sample")
  expect(workspaceBackend("macos", "?seeded=590")).toBe("host")
})
