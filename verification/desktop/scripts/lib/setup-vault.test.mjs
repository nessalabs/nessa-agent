import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import { runInNewContext } from "node:vm"
import { unlockRequestedVault, unlockVault } from "./vault.mjs"

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

/**
 * A setup window over a vault in `state`. `prompts` is how many vault dialogs
 * the window draws on its own; onboarding asks for none (#1340), so any is a
 * refusal, and the window never fills or submits a passphrase.
 */
function fixture({ state = "locked", prompts = 0 } = {}) {
  const calls = []
  const page = {
    addInitScript: async () => {},
    goto: async () => {
      calls.push("goto")
    },
    waitForTimeout: async (ms) => {
      calls.push(`settle:${ms}`)
    },
    close: async () => {
      calls.push("close")
    },
    locator: (selector) => {
      if (selector === ".nessa-setup-window")
        return {
          waitFor: async () => {
            calls.push("shell")
          },
        }
      assert.equal(selector, "[data-vault-dialog]")
      return {
        count: async () => {
          calls.push(`dialogs:${prompts}`)
          return prompts
        },
      }
    },
  }
  const client = {
    vault: {
      status: async () => {
        calls.push(`status:${state}`)
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

test("a locked vault: the setup window opens with no vault dialog and asks nothing", async () => {
  const f = fixture()
  assert.equal(await setupWindow(f.run), f.page)
  assert.deepEqual(f.calls, ["install", "goto", "shell", "settle:1500", "dialogs:0"])
})

test("an unlocked vault: the same, and its state is not read or changed", async () => {
  const f = fixture({ state: "unlocked" })
  assert.equal(await setupWindow(f.run), f.page)
  assert.equal(
    f.calls.some((call) => call.startsWith("status:")),
    false,
  )
})

test("a vault dialog drawn by onboarding on its own refuses the window", async () => {
  const f = fixture({ prompts: 1 })
  await assert.rejects(setupWindow(f.run), /opened a vault dialog on its own/)
  // The refused window is closed, not left open for the steps after it.
  assert.equal(f.calls.at(-1), "close")
})
