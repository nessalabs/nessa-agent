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
