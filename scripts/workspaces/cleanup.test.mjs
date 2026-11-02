import { test } from "node:test"
import assert from "node:assert/strict"
import { assignmentEvidence, reconcileAssignments } from "./cleanup.mjs"

test("provisioning and refused unowned targets retain uncertainty without authorizing deletion", () => {
  const provisioning = { state: "provisioning", thread: "child" }
  const assigned = { state: "assigned", thread: "child", vm: "owned-vm" }
  const refused = { state: "refused", thread: "child", vm: "other-vm" }
  for (const transitions of [
    [],
    [provisioning],
    [provisioning, refused],
    [provisioning, assigned, refused],
  ]) {
    const receipt = assignmentEvidence({ transitions })
    assert(receipt.unresolved)
    assert.deepEqual(
      receipt.owned,
      transitions.filter((entry) => entry === assigned),
    )
  }
  for (const transitions of [
    [provisioning, assigned],
    [provisioning, assigned, { ...refused, vm: "owned-vm" }],
  ]) {
    const receipt = assignmentEvidence({ transitions })
    assert.equal(receipt.unresolved, false)
    assert.deepEqual(receipt.owned, [assigned])
  }
})

test("final receipts include allocations arriving during coordinator close and shutdown", async () => {
  const receipts = new Map([["main-vm", "main"]])
  const owned = new Map()
  const order = []
  let readable = true
  await reconcileAssignments({
    conversations: new Set(["main"]),
    client: {
      conversation: {
        close: async () => {
          receipts.set("child-vm", "child")
        },
      },
      close: () => order.push("client closed"),
    },
    gateway: {
      stop: async (beforeRemove) => {
        receipts.set("late-vm", "late")
        order.push("gateway stopped")
        await beforeRemove({ stopped: true, exitCode: 0, signal: null })
        readable = false
      },
    },
    assignments: () => {
      assert(readable, "ledger removed before inspection")
      order.push("final ledger")
      for (const [vm, thread] of receipts) owned.set(vm, thread)
    },
    unconfirmed: assert.fail,
  })
  const deleted = [...owned.keys()]
  assert.deepEqual(deleted, ["main-vm", "child-vm", "late-vm"])
  assert.deepEqual(order, ["client closed", "gateway stopped", "final ledger"])
})

test("failed conversation close still quiesces and reconciles known ownership", async () => {
  const failures = []
  let reconciled = false
  await reconcileAssignments({
    conversations: new Set(["main"]),
    client: {
      conversation: {
        close: async () => {
          throw new Error("close uncertain")
        },
      },
      close: () => {},
    },
    gateway: {
      stop: async (beforeRemove) =>
        beforeRemove({ stopped: false, exitCode: null, signal: null }),
    },
    assignments: () => {
      reconciled = true
    },
    unconfirmed: (entry) => failures.push(entry),
  })
  assert(reconciled)
  assert.deepEqual(
    failures.map((entry) => entry.result),
    ["close unconfirmed", "gateway shutdown unconfirmed"],
  )
})

test("forced termination and nonzero exit remain unconfirmed despite known final receipts", async () => {
  for (const status of [
    { stopped: true, exitCode: null, signal: "SIGKILL" },
    { stopped: true, exitCode: 27, signal: null },
  ]) {
    const failures = []
    let reconciled = false
    await reconcileAssignments({
      conversations: [],
      gateway: { stop: async (beforeRemove) => beforeRemove(status) },
      assignments: () => {
        reconciled = true
      },
      unconfirmed: (entry) => failures.push(entry),
    })
    assert(reconciled)
    assert.equal(failures.length, 1)
    assert.equal(failures[0].result, "gateway shutdown unconfirmed")
  }
})

test("client transport failure still stops the allocator before inspecting receipts", async () => {
  const failures = []
  const order = []
  await reconcileAssignments({
    conversations: [],
    client: {
      close: () => {
        throw new Error("transport close failed")
      },
    },
    gateway: {
      stop: async (beforeRemove) => {
        order.push("stopped")
        await beforeRemove({ stopped: true, exitCode: 0, signal: null })
      },
    },
    assignments: () => order.push("inspected"),
    unconfirmed: (entry) => failures.push(entry),
  })
  assert.deepEqual(order, ["stopped", "inspected"])
  assert.equal(failures[0].result, "client transport close unconfirmed")
})

test("unreadable final ledger propagates as unresolved ownership", async () => {
  await assert.rejects(
    reconcileAssignments({
      conversations: [],
      gateway: {
        stop: async (beforeRemove) =>
          beforeRemove({ stopped: true, exitCode: 0, signal: null }),
      },
      assignments: () => {
        throw new Error("ledger unavailable")
      },
      unconfirmed: assert.fail,
    }),
    /ledger unavailable/,
  )
})
