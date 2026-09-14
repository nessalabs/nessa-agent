import { afterEach, expect, it, vi } from "vitest"
import { createDependencies } from "./dependencies"
import { makeStore } from "../store"
import {
  attachFiles,
  closeConversation,
  openConversation,
  removeFile,
  setActive,
} from "../conversation/adapters/store/slice"

afterEach(() => vi.restoreAllMocks())

it("releases removed, closed and rejected resources but keeps inactive-tab previews", () => {
  let index = 0
  vi.spyOn(URL, "createObjectURL").mockImplementation(() => `blob:${++index}`)
  const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {})
  const dependencies = createDependencies()
  const store = makeStore(dependencies)
  const [a, b] = dependencies.attachments.add([
    new File(["a"], "a.txt"),
    new File(["b"], "b.txt"),
  ])
  store.dispatch(attachFiles({ files: [a!, b!], conversationId: "c0" }))
  store.dispatch(openConversation())
  expect(revoke).not.toHaveBeenCalled()
  store.dispatch(setActive("c0"))
  store.dispatch(removeFile(a!.id))
  expect(revoke).toHaveBeenCalledExactlyOnceWith("blob:1")
  store.dispatch(closeConversation("c0"))
  expect(revoke).toHaveBeenLastCalledWith("blob:2")
  const rejected = dependencies.attachments.add([new File(["x"], "x.txt")])
  store.dispatch(attachFiles({ files: rejected, conversationId: "closed" }))
  expect(revoke).toHaveBeenLastCalledWith("blob:3")
  expect(revoke).toHaveBeenCalledTimes(3)
})
