import { describe, expect, it } from "vitest"

import { createLinux } from "./linux.mjs"
import { createMacos } from "./macos.mjs"
import { createWindows } from "./windows.mjs"
import { parseArgs, pnpmBin, FAST_PROFILE, launch } from "./run.mjs"
import { resolveLaunch } from "./resolve.mjs"

describe("resolveLaunch", () => {
  it("injects linux on linux", () => {
    expect(resolveLaunch("linux").kind).toBe("linux")
  })

  it("injects macos on darwin", () => {
    expect(resolveLaunch("darwin").kind).toBe("macos")
  })

  it("injects windows on win32", () => {
    expect(resolveLaunch("win32").kind).toBe("windows")
  })

  it("refuses an unknown platform", () => {
    expect(() => resolveLaunch("aix")).toThrow(/no launch host/)
  })
})

describe("linux host", () => {
  it("is a .deb and needs a display", () => {
    const host = createLinux({ hasDrm: () => true, pkgConfig: () => true })
    expect(host.fastBundle).toBe("deb")
    expect(host.releaseBundle).toBe("deb")
    expect(host.hasGui({ DISPLAY: ":1" })).toBe(true)
    expect(host.hasGui({ WAYLAND_DISPLAY: "wayland-0" })).toBe(true)
    expect(host.hasGui({})).toBe(false)
    expect(host.hasGui({ DISPLAY: ":1", CI: "1" })).toBe(false)
  })

  it("disables WebKit DMA-BUF when there is no DRM device", () => {
    const host = createLinux({ hasDrm: () => false, pkgConfig: () => true })
    expect(host.prepareEnv({})).toEqual({
      WEBKIT_DISABLE_DMABUF_RENDERER: "1",
      WEBKIT_DISABLE_COMPOSITING_MODE: "1",
    })
  })

  it("leaves a chosen WebKit path alone", () => {
    const host = createLinux({ hasDrm: () => false, pkgConfig: () => true })
    expect(host.prepareEnv({ WEBKIT_DISABLE_DMABUF_RENDERER: "0" })).toEqual({})
  })

  it("does not touch WebKit when a DRM device exists", () => {
    const host = createLinux({ hasDrm: () => true, pkgConfig: () => true })
    expect(host.prepareEnv({})).toEqual({})
  })

  it("names the apt line when pkg-config cannot see WebKitGTK", () => {
    const host = createLinux({ hasDrm: () => true, pkgConfig: () => false })
    expect(host.missingNative()).toMatch(/libwebkit2gtk-4\.1-dev/)
  })
})

describe("macos host", () => {
  it("is a .app and has a GUI outside CI", () => {
    const host = createMacos({ xcodeSelect: () => true })
    expect(host.fastBundle).toBe("app")
    expect(host.releaseBundle).toBe("dmg")
    expect(host.hasGui({})).toBe(true)
    expect(host.hasGui({ CI: "1" })).toBe(false)
    expect(host.prepareEnv({})).toEqual({})
    expect(host.missingNative()).toBeNull()
  })

  it("names xcode-select when the CLT are missing", () => {
    const host = createMacos({ xcodeSelect: () => false })
    expect(host.missingNative()).toMatch(/xcode-select --install/)
  })
})

describe("windows host", () => {
  it("is nsis and has a GUI outside CI", () => {
    const host = createWindows()
    expect(host.kind).toBe("windows")
    expect(host.fastBundle).toBe("nsis")
    expect(host.releaseBundle).toBe("nsis")
    expect(host.hasGui({})).toBe(true)
    expect(host.hasGui({ CI: "1" })).toBe(false)
    expect(host.hasGui({ SESSIONNAME: "Services" })).toBe(false)
    expect(host.hasGui({ SESSIONNAME: "Console" })).toBe(true)
    expect(host.prepareEnv({})).toEqual({})
    expect(host.missingNative()).toBeNull()
  })
})

describe("parseArgs", () => {
  it("defaults to the desktop app", () => {
    expect(parseArgs([])).toEqual({ mode: "app" })
  })

  it("maps --web, --fast, --dev, and --release", () => {
    expect(parseArgs(["--web"])).toEqual({ mode: "web" })
    expect(parseArgs(["--fast"])).toEqual({ mode: "fast" })
    expect(parseArgs(["--dev"])).toEqual({ mode: "app" })
    expect(parseArgs(["--release"])).toEqual({ mode: "release" })
  })

  it("treats -h as help", () => {
    expect(parseArgs(["--help"]).mode).toBe("help")
    expect(parseArgs(["-h"]).mode).toBe("help")
  })

  it("rejects unknown flags", () => {
    expect(parseArgs(["--nope"])).toEqual({
      mode: "error",
      error: "unknown option: --nope (try --dev, --web, --fast, or --release)",
    })
  })
})

describe("pnpmBin", () => {
  it("uses pnpm.cmd on Windows so spawn can find it", () => {
    expect(pnpmBin("win32")).toBe("pnpm.cmd")
    expect(pnpmBin("linux")).toBe("pnpm")
    expect(pnpmBin("darwin")).toBe("pnpm")
  })
})

describe("launch", () => {
  it("falls back to the web UI when Linux has no display", () => {
    const calls: string[][] = []
    const logs: string[] = []
    const host = createLinux({ hasDrm: () => true, pkgConfig: () => true })
    const status = launch("app", {
      host,
      env: {},
      log: (msg) => logs.push(msg),
      run: (args) => {
        calls.push(args)
        return 0
      },
    })
    expect(status).toBe(0)
    expect(logs[0]).toMatch(/no GUI/)
    expect(calls).toEqual([["dev"]])
  })

  it("opens the browser when a GUI exists and --web is asked", () => {
    const calls: string[][] = []
    const host = createMacos({ xcodeSelect: () => true })
    launch("web", {
      host,
      env: {},
      run: (args) => {
        calls.push(args)
        return 0
      },
    })
    expect(calls).toEqual([["dev", "--", "--open"]])
  })

  it("runs tauri dev after Linux native prep", () => {
    const calls: { args: string[]; env: NodeJS.ProcessEnv }[] = []
    const host = createLinux({ hasDrm: () => false, pkgConfig: () => true })
    launch("app", {
      host,
      env: { DISPLAY: ":1" },
      run: (args, opts) => {
        calls.push({ args, env: opts?.env ?? {} })
        return 0
      },
    })
    expect(calls[0]?.args).toEqual(["app"])
    expect(calls[0]?.env.WEBKIT_DISABLE_DMABUF_RENDERER).toBe("1")
    expect(calls[0]?.env.DISPLAY).toBe(":1")
  })

  it("refuses to compile when Linux packages are missing", () => {
    const logs: string[] = []
    const host = createLinux({ hasDrm: () => true, pkgConfig: () => false })
    const status = launch("app", {
      host,
      env: { DISPLAY: ":1" },
      log: (msg) => logs.push(msg),
      run: () => {
        throw new Error("must not spawn")
      },
    })
    expect(status).toBe(1)
    expect(logs.join("\n")).toMatch(/libwebkit2gtk-4\.1-dev/)
  })

  it("asks Tauri for the host bundle on --fast", () => {
    const calls: { args: string[]; env: NodeJS.ProcessEnv }[] = []
    const host = createWindows()
    launch("fast", {
      host,
      env: {},
      run: (args, opts) => {
        calls.push({ args, env: opts?.env ?? {} })
        return 0
      },
    })
    expect(calls[0]?.args).toEqual(["exec", "tauri", "build", "--bundles", "nsis"])
    expect(calls[0]?.env).toMatchObject(FAST_PROFILE)
  })

  it("asks for a .deb on Linux --fast", () => {
    const calls: string[][] = []
    const host = createLinux({ hasDrm: () => true, pkgConfig: () => true })
    launch("fast", {
      host,
      env: { DISPLAY: ":1" },
      run: (args) => {
        calls.push(args)
        return 0
      },
    })
    expect(calls[0]).toEqual(["exec", "tauri", "build", "--bundles", "deb"])
  })

  it("asks for a .app on macOS --fast", () => {
    const calls: string[][] = []
    const host = createMacos({ xcodeSelect: () => true })
    launch("fast", {
      host,
      env: {},
      run: (args) => {
        calls.push(args)
        return 0
      },
    })
    expect(calls[0]).toEqual(["exec", "tauri", "build", "--bundles", "app"])
  })

  it("asks for a .dmg on macOS --release without the fast profile", () => {
    const calls: { args: string[]; env: NodeJS.ProcessEnv }[] = []
    const host = createMacos({ xcodeSelect: () => true })
    launch("release", {
      host,
      env: {},
      run: (args, opts) => {
        calls.push({ args, env: opts?.env ?? {} })
        return 0
      },
    })
    expect(calls[0]?.args).toEqual(["exec", "tauri", "build", "--bundles", "dmg"])
    expect(calls[0]?.env.CARGO_PROFILE_RELEASE_LTO).toBeUndefined()
  })

  it("asks for a .deb on Linux --release without the fast profile", () => {
    const calls: { args: string[]; env: NodeJS.ProcessEnv }[] = []
    const host = createLinux({ hasDrm: () => true, pkgConfig: () => true })
    launch("release", {
      host,
      env: { DISPLAY: ":1" },
      run: (args, opts) => {
        calls.push({ args, env: opts?.env ?? {} })
        return 0
      },
    })
    expect(calls[0]?.args).toEqual(["exec", "tauri", "build", "--bundles", "deb"])
    expect(calls[0]?.env.CARGO_PROFILE_RELEASE_OPT_LEVEL).toBeUndefined()
  })
})
