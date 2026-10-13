/** Bare Node contract, also run where frontend dependencies are absent. */
import assert from "node:assert/strict"
import test from "node:test"
import { conversationImportAllowed } from "./conversation-boundary.mjs"

test("the pure resource contract is public while model and internal paths remain private", () => {
  const from = "src/panel/adapters/attachment-resources.ts"
  for (const path of [
    "../../conversation",
    "../../conversation/index",
    "../../conversation/resources",
  ])
    assert.equal(conversationImportAllowed(from, path), true, path)
  for (const path of [
    "../../conversation/model",
    "../../conversation/model/attachments",
    "../../conversation/resources/other",
    "../../conversation/application/queries/attachment-resources",
    "../../conversation/adapters/store/slice",
  ])
    assert.equal(conversationImportAllowed(from, path), false, path)
})

test("composition exceptions remain scoped to their actual owners", () => {
  assert.equal(
    conversationImportAllowed("src/store.ts", "./conversation/adapters/store/slice"),
    true,
  )
  assert.equal(
    conversationImportAllowed("src/icon-preview.tsx", "./conversation/model"),
    true,
  )
  assert.equal(
    conversationImportAllowed("src/panel/ui/app.tsx", "../../conversation/model"),
    false,
  )
  assert.equal(
    conversationImportAllowed(
      "src/composition/dependencies.ts",
      "../conversation/application/ports",
      true,
    ),
    true,
  )
  assert.equal(
    conversationImportAllowed(
      "src/panel/adapters/attachment-resources.ts",
      "../../conversation/application/ports",
    ),
    false,
  )
})
