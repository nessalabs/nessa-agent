import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import { runInNewContext } from "node:vm"
import { unlockRequestedVault, unlockVault, VAULT_PASSPHRASE } from "./vault.mjs"

// Exercise the actual window initializer without its CLI or browser main.
const source = readFileSync(
  new URL("../conversation-stress.mjs", import.meta.url),
  "utf8",
)
const start = source.indexOf("async function setupWindow(")
const end = source.indexOf("\n/**", start)
assert.ok(start >= 0 && end > start)
const setupWindow = runInNewContext(`(${source.slice(start, end)})`, {
  URL,
  gatewayHost() {},
  unlockRequestedVault,
  unlockVault,
})

function fixture({
  state = "locked",
  statusError,
  promptError,
  staysLocked = false,
} = {}) {
  const calls = []
  let visible = false
  const dialog = {
    isVisible: async () => visible,
    waitFor: async (options) => {
      if (options?.state === "hidden") {
        calls.push("hidden")
        assert.equal(visible, false)
        return
      }
      calls.push("prompt")
      assert.equal(options?.timeout, 15_000)
      if (promptError) throw promptError
      // The authoritative status can finish before React paints its dialog.
      // isVisible remains false until this actual readiness wait yields.
      await Promise.resolve()
      visible = true
    },
    getByLabel: (name) => {
      assert.equal(name, "Passphrase")
      return {
        fill: async (value) => {
          assert.equal(visible, true)
          assert.equal(value, VAULT_PASSPHRASE)
          calls.push("fill")
        },
      }
    },
    getByRole: (role, { name }) => {
      assert.equal(role, "button")
      assert.equal(name, "Unlock")
      return {
        click: async () => {
          calls.push("unlock")
          visible = false
          if (!staysLocked) state = "unlocked"
        },
      }
    },
  }
  const page = {
    addInitScript: async () => {},
    goto: async () => {
      calls.push("goto")
    },
    locator: (selector) => {
      if (selector === ".nessa-setup-window")
        return {
          waitFor: async () => {
            calls.push("shell")
          },
        }
      assert.equal(selector, '[data-vault-dialog="unlock"]')
      return dialog
    },
    getByRole: (role, { name }) => {
      assert.equal(role, "dialog")
      assert.equal(name, "Unlock Nessa")
      return { evaluate: async () => true }
    },
  }
  const client = {
    vault: {
      status: async () => {
        calls.push(`status:${state}`)
        if (statusError) throw statusError
        return { state }
      },
    },
  }
  const run = {
    client,
    page: {
      context: () => ({ newPage: async () => page }),
      url: () => "http://127.0.0.1:1/desktop.html",
    },
    stack: {
      vault: {
        install: async () => {
          calls.push("install")
        },
      },
    },
  }
  return { run, page, calls }
}

test("setup waits for a late locked-vault prompt and confirms the actual unlock", async () => {
  const f = fixture()
  assert.equal(await setupWindow(f.run), f.page)
  assert.deepEqual(f.calls, [
    "install",
    "goto",
    "shell",
    "status:locked",
    "prompt",
    "fill",
    "unlock",
    "hidden",
    "status:unlocked",
  ])
})

test("already unlocked setup performs no passphrase or unlock action", async () => {
  const f = fixture({ state: "unlocked" })
  assert.equal(await setupWindow(f.run), f.page)
  assert.deepEqual(f.calls, ["install", "goto", "shell", "status:unlocked"])
})

test("unavailable status refuses setup readiness before any unlock", async () => {
  const f = fixture({ statusError: new Error("status unavailable") })
  await assert.rejects(setupWindow(f.run), /status unavailable/)
  assert.equal(f.calls.includes("unlock"), false)
})

test("a missing prompt refuses inside the unchanged fifteen-second ceiling", async () => {
  const f = fixture({ promptError: new Error("prompt unavailable") })
  await assert.rejects(setupWindow(f.run), /prompt unavailable/)
  assert.equal(f.calls.includes("unlock"), false)
})

test("a completed click without authoritative unlock is not successful readiness", async () => {
  const f = fixture({ staysLocked: true })
  await assert.rejects(setupWindow(f.run), /did not unlock/)
  assert.equal(f.calls.filter((c) => c === "unlock").length, 1)
})
