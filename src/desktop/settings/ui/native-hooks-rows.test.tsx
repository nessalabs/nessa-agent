// @vitest-environment jsdom
/**
 * Settings › Native hooks: the rows rest without a gateway, and with
 * one they show the catalog's choice and send a change.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { NativeHooksGateway } from "../adapters/native-hooks-gateway"
import { NativeHooksProvider, NativeHooksRows } from "./native-hooks-rows"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

const row = (id: string) => host.querySelector(`[data-setting="${id}"]`)
const toggle = (id: string) =>
  row(id)?.querySelector<HTMLButtonElement>('[role="switch"]')

it("rests both rows when this window has no gateway", async () => {
  await act(async () => root.render(<NativeHooksRows />))
  for (const id of ["claude-native-hooks", "codex-native-hooks"]) {
    expect(row(id)?.textContent).toContain("Not available in this window.")
    expect(row(id)?.getAttribute("data-disabled")).not.toBeNull()
    expect(toggle(id)).toBeNull()
  }
  expect(host.textContent).toContain(
    "Hooks can approve, deny, or change a tool call and its result without Nessa asking, and can run commands when the agent starts.",
  )
})

it("shows the catalog's choice and sends a change for that harness", async () => {
  const sent: { agent: string; enabled: boolean }[] = []
  let choice = { claude: false, codex: true }
  const gateway: NativeHooksGateway = {
    read: () => Promise.resolve({ ok: true, choice: { ...choice } }),
    set: (agent, enabled) => {
      sent.push({ agent, enabled })
      choice = { ...choice, [agent]: enabled }
      return Promise.resolve({ ok: true, agent, enabled })
    },
  }
  await act(async () =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    ),
  )
  await act(async () => {})
  expect(toggle("claude-native-hooks")?.getAttribute("aria-checked")).toBe("false")
  expect(toggle("codex-native-hooks")?.getAttribute("aria-checked")).toBe("true")
  await act(async () => toggle("claude-native-hooks")?.click())
  expect(sent).toEqual([{ agent: "claude", enabled: true }])
  expect(toggle("claude-native-hooks")?.getAttribute("aria-checked")).toBe("true")
})

it("keeps the previous choice when the gateway refuses the change", async () => {
  const gateway: NativeHooksGateway = {
    read: () => Promise.resolve({ ok: true, choice: { claude: false, codex: false } }),
    set: () => Promise.resolve({ ok: false }),
  }
  await act(async () =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    ),
  )
  await act(async () => {})
  await act(async () => toggle("codex-native-hooks")?.click())
  expect(toggle("codex-native-hooks")?.getAttribute("aria-checked")).toBe("false")
  expect(row("codex-native-hooks")?.textContent).toContain("Could not change this.")
})

it("shows the choice a lost response left in effect", async () => {
  let choice = { claude: false, codex: false }
  const gateway: NativeHooksGateway = {
    read: () => Promise.resolve({ ok: true, choice: { ...choice } }),
    set: (_agent, enabled) => {
      choice = { ...choice, codex: enabled }
      return Promise.resolve({ ok: false })
    },
  }
  await act(async () =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    ),
  )
  await act(async () => {})
  await act(async () => toggle("codex-native-hooks")?.click())
  expect(toggle("codex-native-hooks")?.getAttribute("aria-checked")).toBe("true")
  expect(row("codex-native-hooks")?.textContent).toContain("Could not change this.")
})

it("keeps the stored choice when the agent is not in the catalog", async () => {
  const gateway: NativeHooksGateway = {
    read: () =>
      Promise.resolve({
        ok: true,
        choice: {
          claude: true,
          codex: false,
          claudeConfigured: false,
          codexConfigured: true,
        },
      }),
    set: () => Promise.resolve({ ok: false }),
  }
  await act(async () =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    ),
  )
  await act(async () => {})
  expect(toggle("claude-native-hooks")?.getAttribute("aria-checked")).toBe("true")
  expect(row("claude-native-hooks")?.textContent).toContain("This agent isn't set up.")
  expect(row("claude-native-hooks")?.textContent).toContain("only for projects you trust")
})

it("reads the choice again when the window regains focus or becomes visible", async () => {
  let choice = { claude: false, codex: false }
  const gateway: NativeHooksGateway = {
    read: () => Promise.resolve({ ok: true, choice: { ...choice } }),
    set: () => Promise.resolve({ ok: false }),
  }
  await act(async () =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    ),
  )
  await act(async () => {})
  expect(toggle("claude-native-hooks")?.getAttribute("aria-checked")).toBe("false")
  // Another client turns Claude's hooks on while Native hooks stays open.
  choice = { claude: true, codex: false }
  await act(async () => window.dispatchEvent(new Event("focus")))
  expect(toggle("claude-native-hooks")?.getAttribute("aria-checked")).toBe("true")
  choice = { claude: true, codex: true }
  await act(async () => document.dispatchEvent(new Event("visibilitychange")))
  expect(toggle("codex-native-hooks")?.getAttribute("aria-checked")).toBe("true")
})

it("lets only the latest of overlapping reads answer", async () => {
  const pending: ((choice: { claude: boolean; codex: boolean }) => void)[] = []
  const gateway: NativeHooksGateway = {
    read: () =>
      new Promise((resolve) => pending.push((choice) => resolve({ ok: true, choice }))),
    set: () => Promise.resolve({ ok: false }),
  }
  await act(async () =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    ),
  )
  await act(async () => pending[0]?.({ claude: false, codex: false }))
  await act(async () => window.dispatchEvent(new Event("focus")))
  await act(async () => window.dispatchEvent(new Event("focus")))
  await act(async () => pending[2]?.({ claude: true, codex: false }))
  await act(async () => pending[1]?.({ claude: false, codex: false }))
  expect(toggle("claude-native-hooks")?.getAttribute("aria-checked")).toBe("true")
})

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

it("admits one native-hook write for same-task clicks before the disabled render", async () => {
  const requests: string[] = []
  const gateway: NativeHooksGateway = {
    read: async () => ({ ok: true, choice: { claude: false, codex: false } }),
    set: (_agent, _enabled, requestId) => {
      requests.push(requestId)
      return new Promise(() => {})
    },
  }
  await act(async () =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    ),
  )
  const button = requiredToggle("claude-native-hooks")
  await act(async () => {
    button.click()
    button.click()
  })
  expect(requests).toHaveLength(1)
})

it("retires A's pending save so replacement source B reads and keeps its own choice", async () => {
  const save = deferred<Awaited<ReturnType<NativeHooksGateway["set"]>>>()
  let aReads = 0
  let bReads = 0
  const a: NativeHooksGateway = {
    read: async () => {
      aReads++
      return { ok: true, choice: { claude: aReads > 1, codex: false } }
    },
    set: () => save.promise,
  }
  const b: NativeHooksGateway = {
    read: async () => {
      bReads++
      return { ok: true, choice: { claude: false, codex: false } }
    },
    set: async () => ({ ok: false }),
  }
  const render = (gateway: NativeHooksGateway) =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    )
  await act(async () => render(a))
  await act(async () => requiredToggle("claude-native-hooks").click())
  await act(async () => render(b))
  await act(async () => save.resolve({ ok: true, agent: "claude", enabled: true }))
  expect(bReads).toBe(1)
  expect(aReads).toBe(1)
  expect(toggle("claude-native-hooks")?.getAttribute("aria-checked")).toBe("false")
})

it("retains one save through its reread and defers focus/visibility refreshes", async () => {
  const saved = deferred<Awaited<ReturnType<NativeHooksGateway["set"]>>>()
  const reread = deferred<Awaited<ReturnType<NativeHooksGateway["read"]>>>()
  const requests: string[] = []
  let reads = 0
  const gateway: NativeHooksGateway = {
    read: () =>
      ++reads === 1
        ? Promise.resolve({ ok: true, choice: { claude: false, codex: false } })
        : reread.promise,
    set: (_agent, _enabled, id) => {
      requests.push(id)
      return saved.promise
    },
  }
  await act(async () =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    ),
  )
  await act(async () => requiredToggle("claude-native-hooks").click())
  await act(async () => requiredToggle("codex-native-hooks").click())
  expect(requests).toHaveLength(1)
  expect(requiredToggle("codex-native-hooks").disabled).toBe(true)
  await act(async () => {
    window.dispatchEvent(new Event("focus"))
    document.dispatchEvent(new Event("visibilitychange"))
    saved.resolve({ ok: true, agent: "claude", enabled: true })
  })
  expect(reads).toBe(2)
  await act(async () => window.dispatchEvent(new Event("focus")))
  expect(reads).toBe(2)
  expect(requiredToggle("claude-native-hooks").disabled).toBe(true)
  await act(async () =>
    reread.resolve({ ok: true, choice: { claude: true, codex: false } }),
  )
  expect(requiredToggle("codex-native-hooks").disabled).toBe(false)
  await act(async () => requiredToggle("codex-native-hooks").click())
  expect(requests).toHaveLength(2)
  expect(new Set(requests).size).toBe(2)
})

it("cannot let a retired reread publish or unlock the replacement source's save", async () => {
  const oldRead = deferred<Awaited<ReturnType<NativeHooksGateway["read"]>>>()
  const newSave = deferred<Awaited<ReturnType<NativeHooksGateway["set"]>>>()
  let aReads = 0
  let bWrites = 0
  const a: NativeHooksGateway = {
    read: () =>
      ++aReads === 1
        ? Promise.resolve({ ok: true, choice: { claude: false, codex: false } })
        : oldRead.promise,
    set: async (agent, enabled) => ({ ok: true, agent, enabled }),
  }
  const b: NativeHooksGateway = {
    read: async () => ({ ok: true, choice: { claude: false, codex: false } }),
    set: () => {
      bWrites++
      return newSave.promise
    },
  }
  const render = (gateway: NativeHooksGateway) =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    )
  await act(async () => render(a))
  await act(async () => requiredToggle("claude-native-hooks").click())
  expect(aReads).toBe(2)
  await act(async () => render(b))
  await act(async () => requiredToggle("codex-native-hooks").click())
  await act(async () =>
    oldRead.resolve({ ok: true, choice: { claude: true, codex: false } }),
  )
  expect(requiredToggle("claude-native-hooks").getAttribute("aria-checked")).toBe("false")
  expect(requiredToggle("codex-native-hooks").getAttribute("aria-checked")).toBe("true")
  expect(requiredToggle("claude-native-hooks").disabled).toBe(true)
  await act(async () => requiredToggle("claude-native-hooks").click())
  expect(bWrites).toBe(1)
  await act(async () => newSave.resolve({ ok: false }))
  expect(requiredToggle("codex-native-hooks").disabled).toBe(false)
})

it("does not start a reread after the saving view unmounts", async () => {
  const saved = deferred<Awaited<ReturnType<NativeHooksGateway["set"]>>>()
  let reads = 0
  const gateway: NativeHooksGateway = {
    read: async () => {
      reads++
      return { ok: true, choice: { claude: false, codex: false } }
    },
    set: () => saved.promise,
  }
  await act(async () =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    ),
  )
  await act(async () => requiredToggle("claude-native-hooks").click())
  await act(async () => root.render(null))
  await act(async () => saved.resolve({ ok: true, agent: "claude", enabled: true }))
  window.dispatchEvent(new Event("focus"))
  document.dispatchEvent(new Event("visibilitychange"))
  expect(reads).toBe(1)
})

it.each([true, false])(
  "uses only a matching acknowledged choice when rereading fails (matching=%s)",
  async (matching) => {
    let reads = 0
    const gateway: NativeHooksGateway = {
      read: async () =>
        ++reads === 1
          ? { ok: true, choice: { claude: false, codex: true } }
          : { ok: false },
      set: async () => ({
        ok: true,
        agent: matching ? "claude" : "codex",
        enabled: true,
      }),
    }
    await act(async () =>
      root.render(
        <NativeHooksProvider gateway={gateway}>
          <NativeHooksRows />
        </NativeHooksProvider>,
      ),
    )
    await act(async () => requiredToggle("claude-native-hooks").click())
    if (matching) {
      expect(requiredToggle("claude-native-hooks").getAttribute("aria-checked")).toBe(
        "true",
      )
      expect(requiredToggle("claude-native-hooks").disabled).toBe(false)
    } else {
      expect(toggle("claude-native-hooks")).toBeNull()
      expect(requiredRow("claude-native-hooks").textContent).toContain(
        "Could not read this choice.",
      )
    }
  },
)

function requiredToggle(id: string) {
  const button = toggle(id)
  if (!button) throw new Error(`Missing native-hook switch ${id}`)
  return button
}

function requiredRow(id: string) {
  const element = row(id)
  if (!element) throw new Error(`Missing native-hook row ${id}`)
  return element
}
