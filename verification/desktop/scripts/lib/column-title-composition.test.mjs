import assert from "node:assert/strict"
import { test } from "node:test"
import { sidebarTitleCompositionFailures } from "./column-title-composition.mjs"

const open = {
  workspaces: 1,
  sidebars: 1,
  sidebarId: "workspace-sidebar",
  variant: "tree",
  listState: null,
  state: "open",
  peek: false,
  navigations: 1,
  channels: 3,
  branches: 3,
  sessions: 9,
  sessionLists: 0,
  titles: 0,
  inert: false,
  ariaHidden: null,
  visible: true,
}

test("positive tree identities and consistent open, folded and reveal facts hold", () => {
  for (const state of [
    open,
    { ...open, ariaHidden: "false" },
    { ...open, state: "closed", inert: true, ariaHidden: "true", visible: false },
    { ...open, state: "closed", peek: true },
    {
      ...open,
      state: "closed",
      peek: true,
      inert: true,
      ariaHidden: "true",
      visible: false,
    },
  ])
    assert.deepEqual(sidebarTitleCompositionFailures(state), [])
})

test("canonical guards can precede parking without exposing the folded tree", () => {
  for (const change of [
    { peek: true, visible: true },
    { peek: true, visible: false },
    { peek: false, visible: false },
  ])
    assert.deepEqual(
      sidebarTitleCompositionFailures({
        ...open,
        state: "closed",
        ariaHidden: "true",
        ...change,
      }),
      [],
    )
})

test("missing, inherited, non-finite and wrong-type observations cannot qualify", () => {
  for (const value of [
    null,
    undefined,
    false,
    "sidebar",
    {},
    Object.create(open),
    Object.assign([], open),
  ])
    assert.ok(sidebarTitleCompositionFailures(value).length)
  for (const key of Object.keys(open)) {
    const missing = { ...open }
    delete missing[key]
    assert.ok(sidebarTitleCompositionFailures(missing).length, key)
    const inherited = Object.assign(Object.create({ [key]: open[key] }), open)
    delete inherited[key]
    assert.ok(sidebarTitleCompositionFailures(inherited).length, key)
  }
  for (const key of [
    "workspaces",
    "sidebars",
    "navigations",
    "channels",
    "branches",
    "sessions",
    "sessionLists",
    "titles",
  ])
    for (const value of [NaN, Infinity, -1, 3.5, "3", null])
      assert.ok(sidebarTitleCompositionFailures({ ...open, [key]: value }).length, key)
  for (const key of ["peek", "inert", "visible"])
    for (const value of [null, "false", 0, {}, []])
      assert.ok(sidebarTitleCompositionFailures({ ...open, [key]: value }).length, key)
})

test("an empty title query cannot replace the actual workspace and sample tree", () => {
  for (const [key, values] of [
    ["workspaces", [0, 2]],
    ["sidebars", [0, 2]],
    ["sidebarId", [null, "other-sidebar"]],
    ["variant", [null, "channels", "constructor"]],
    ["navigations", [0, 2]],
    ["channels", [0]],
    ["branches", [0]],
    ["sessions", [0]],
  ])
    for (const value of values)
      assert.ok(sidebarTitleCompositionFailures({ ...open, [key]: value }).length, key)
})

test("unexpected list state, a mounted list or a hidden real title still fail", () => {
  for (const change of [{ listState: "closed" }, { sessionLists: 1 }, { titles: 1 }])
    assert.ok(sidebarTitleCompositionFailures({ ...open, ...change }).length)
})

test("fold, paint and accessibility facts must describe one possible observation", () => {
  for (const change of [
    { state: null },
    { state: "constructor" },
    { ariaHidden: "unknown" },
    { inert: true },
    { ariaHidden: "true" },
    { inert: true, ariaHidden: "true" },
    { visible: false },
    { state: "closed" },
    { state: "closed", inert: true, ariaHidden: "true" },
    { state: "closed", peek: true, visible: false },
    { state: "closed", peek: true, inert: true },
  ])
    assert.ok(
      sidebarTitleCompositionFailures({ ...open, ...change }).length,
      JSON.stringify(change),
    )
})
