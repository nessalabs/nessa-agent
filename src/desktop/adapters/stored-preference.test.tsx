// @vitest-environment jsdom
/**
 * The preference every reader in the window shares: remembered where storage
 * allows, applied at once everywhere, and falling back rather than trusting a
 * stored value this build does not know.
 */
import { act, StrictMode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { storedPreference } from "./stored-preference"

/** Storage the test controls: a map, which can be made to refuse reads or writes. */
const stored = new Map<string, string>()
const refuse = { read: false, write: false }
const storage = {
  getItem(key: string) {
    if (refuse.read) throw new Error("denied")
    return stored.get(key) ?? null
  },
  setItem(key: string, value: string) {
    if (refuse.write) throw new Error("quota")
    stored.set(key, value)
  },
}

const colour = storedPreference({
  key: "test.colour",
  event: "test:colour",
  parse: (value) => (value === "red" || value === "blue" ? value : "grey"),
  storage: () => storage,
})

let root: Root
let host: HTMLDivElement
const seen: string[][] = [[], []]
let choose: (next: "red" | "blue" | "grey") => void = () => {}

function Reader({ index }: { index: number }) {
  const [value, setValue] = colour.usePreference()
  seen[index].push(value)
  if (index === 0) choose = setValue
  return null
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  stored.clear()
  refuse.read = false
  refuse.write = false
  seen[0] = []
  seen[1] = []
  host = document.createElement("div")
  root = createRoot(host)
})

afterEach(() => {
  act(() => root.unmount())
})

const mount = () =>
  act(() =>
    root.render(
      <StrictMode>
        <Reader index={0} />
        <Reader index={1} />
      </StrictMode>,
    ),
  )

describe("a stored preference", () => {
  it("starts on the fallback with nothing stored, or with something this build does not know", () => {
    expect(colour.read()).toBe("grey")
    stored.set("test.colour", "constructor")
    expect(colour.read()).toBe("grey")
    stored.set("test.colour", "red")
    expect(colour.read()).toBe("red")
  })

  it("remembers a choice and every reader in the window follows it at once", () => {
    mount()
    act(() => choose("blue"))
    expect(stored.get("test.colour")).toBe("blue")
    expect(seen[0].at(-1)).toBe("blue")
    expect(seen[1].at(-1)).toBe("blue")
  })

  it("still applies a choice in this window when storage refuses to keep it", () => {
    refuse.write = true
    mount()
    act(() => choose("red"))
    expect(seen[1].at(-1)).toBe("red")
  })

  it("reads the fallback when storage cannot be read at all", () => {
    refuse.read = true
    expect(colour.read()).toBe("grey")
  })

  it("follows another window's change to its own key, and ignores other keys", () => {
    mount()
    stored.set("test.colour", "red")
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", { key: "unrelated" }))
    })
    expect(seen[1].at(-1)).toBe("grey")
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", { key: "test.colour" }))
    })
    expect(seen[1].at(-1)).toBe("red")
  })
})
