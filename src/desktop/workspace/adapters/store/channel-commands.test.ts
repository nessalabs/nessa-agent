/**
 * The "New channel" flow's commands over a store: what it asks, in what
 * order, what a Create sends, and what a made or refused channel leaves.
 */
import { describe, expect, it } from "vitest"
import { makeDesktopStore } from "../../../store"
import {
  ChannelCreationError,
  type ChannelSource,
  type DroppedPath,
  type PathDropWatch,
  type PathPicker,
} from "../../application/ports"
import { createQuestionFlights } from "../../application/question-flights"
import { channelRules } from "../../model/channel-draft"
import { defaultModel } from "../../model/workspace-index"
import type { Channel } from "../../model/workspace-index"
import { fakeSource, keptFilter, roomyGrid, settle, testIndex } from "../../testing"
import { inMemoryChannels } from "../in-memory/in-memory-channels"
import {
  addChannelResources,
  answerChannelStep,
  askChannelStep,
  chooseChannelFolder,
  closeChannelCreation,
  createChannel,
  draftChannel,
  loadWorkspace,
  openChannelCreation,
  removeChannelResource,
  showContent,
  watchChannelDrops,
} from "./commands"

/** Pickers that answer what the test queued, a cancel when nothing is. */
function queuedPaths() {
  const folders: (string | null)[] = []
  const resources: string[][] = []
  const asked: string[] = []
  const watches = new Set<PathDropWatch>()
  const paths: PathPicker = {
    drops: (watch) => {
      watches.add(watch)
      return () => watches.delete(watch)
    },
    home: async () => "/home/person",
    folder: async () => {
      asked.push("folder")
      return folders.shift() ?? null
    },
    resources: async (kind) => {
      asked.push(kind)
      return resources.shift() ?? []
    },
  }
  return { paths, folders, resources, asked, watches }
}

/** The source's index, which a channel made through `channels()` joins, as a real source's does. */
const index = testIndex()
const listed = index.channels as Channel[]

async function storeWith(options: { channels?: ChannelSource; paths?: PathPicker } = {}) {
  let next = 0
  listed.splice(0, listed.length, ...testIndex().channels)
  const source = fakeSource(index)
  const store = makeDesktopStore({
    workspace: source,
    now: () => 1000,
    newId: () => `id-${++next}`,
    measure: () => roomyGrid,
    overviewFilter: keptFilter(),
    questionFlights: createQuestionFlights(),
    ...options,
  })
  await store.dispatch(loadWorkspace())
  await settle()
  return { store, source }
}

/** A channel source whose channels the store's source lists from then on. */
function channels() {
  return inMemoryChannels({
    add: (made) => {
      const channel = { ...made, sectionId: "starred" }
      listed.push(channel)
      return channel
    },
  })
}

const flowOf = (store: Awaited<ReturnType<typeof storeWith>>["store"]) =>
  store.getState().workspace.channelCreation

describe("opening", () => {
  it("opens on the name, over the panes, where the window can make channels", async () => {
    const { store } = await storeWith({ channels: channels() })
    store.dispatch(showContent({ content: "agents" }))
    store.dispatch(openChannelCreation())
    const flow = flowOf(store)
    expect(flow?.asking).toBe("name")
    expect(flow?.draft.model).toEqual(defaultModel())
    expect(store.getState().workspace.content).toBe("panes")
  })

  it("opens nothing where the window cannot make channels", async () => {
    const { store } = await storeWith()
    store.dispatch(openChannelCreation())
    expect(flowOf(store)).toBeNull()
    expect(store.getState().workspace.channelMaking).toEqual({
      creatable: false,
      pickable: false,
      home: null,
    })
  })

  it("reads the home folder once, where there are pickers", async () => {
    const { paths } = queuedPaths()
    const { store } = await storeWith({ channels: channels(), paths })
    store.dispatch(openChannelCreation())
    await settle()
    expect(store.getState().workspace.channelMaking.home).toBe("/home/person")
  })
})

describe("asking", () => {
  it("asks no folder or resources without pickers, and answers the name only once it is one", async () => {
    const { store } = await storeWith({ channels: channels() })
    store.dispatch(openChannelCreation())
    store.dispatch(answerChannelStep())
    expect(flowOf(store)?.asking).toBe("name")
    store.dispatch(draftChannel({ change: { name: "   " } }))
    store.dispatch(answerChannelStep())
    expect(flowOf(store)?.asking).toBe("name")
    store.dispatch(draftChannel({ change: { name: " design-review " } }))
    store.dispatch(answerChannelStep())
    expect(flowOf(store)?.asking).toBe("placement")
    store.dispatch(answerChannelStep())
    expect(flowOf(store)?.asking).toBe("model")
    store.dispatch(answerChannelStep())
    expect(flowOf(store)?.asking).toBe("notes")
    store.dispatch(answerChannelStep())
    expect(flowOf(store)?.asking).toBeNull()
    expect(flowOf(store)?.answered).toEqual(["name", "placement", "model", "notes"])
  })

  it("asks all five with pickers, and a clicked line asks its step again", async () => {
    const { paths } = queuedPaths()
    const { store } = await storeWith({ channels: channels(), paths })
    store.dispatch(openChannelCreation())
    store.dispatch(draftChannel({ change: { name: "a" } }))
    for (let n = 0; n < 6; n++) store.dispatch(answerChannelStep())
    expect(flowOf(store)?.answered).toEqual([
      "name",
      "folder",
      "placement",
      "model",
      "notes",
      "resources",
    ])
    store.dispatch(askChannelStep({ step: "folder" }))
    expect(flowOf(store)?.asking).toBe("folder")
    store.dispatch(answerChannelStep())
    expect(flowOf(store)?.asking).toBeNull()
  })

  it("leaves a name that is not one unanswered when another step is asked", async () => {
    const { store } = await storeWith({ channels: channels() })
    store.dispatch(openChannelCreation())
    store.dispatch(askChannelStep({ step: "notes" }))
    expect(flowOf(store)?.answered).toEqual([])
    store.dispatch(answerChannelStep())
    expect(flowOf(store)?.asking).toBe("name")
  })

  it("takes the folder and resources the pickers chose; a cancel changes nothing", async () => {
    const { paths, folders, resources, asked } = queuedPaths()
    const { store } = await storeWith({ channels: channels(), paths })
    store.dispatch(openChannelCreation())
    folders.push("/work/site")
    await store.dispatch(chooseChannelFolder())
    expect(flowOf(store)?.draft.folder).toBe("/work/site")
    await store.dispatch(chooseChannelFolder())
    expect(flowOf(store)?.draft.folder).toBe("/work/site")
    resources.push(["/work/a.md", "/work/b.md"], ["/work/docs", "/work/a.md"])
    await store.dispatch(addChannelResources({ kind: "files" }))
    await store.dispatch(addChannelResources({ kind: "folders" }))
    expect(flowOf(store)?.draft.resources).toEqual([
      "/work/a.md",
      "/work/b.md",
      "/work/docs",
    ])
    expect(flowOf(store)?.kinds).toEqual({
      "/work/a.md": "file",
      "/work/b.md": "file",
      "/work/docs": "folder",
    })
    store.dispatch(removeChannelResource({ path: "/work/b.md" }))
    expect(flowOf(store)?.draft.resources).toEqual(["/work/a.md", "/work/docs"])
    expect(asked).toEqual(["folder", "folder", "files", "folders"])
  })

  it("keeps as many resources as a channel holds, and says how many did not fit", async () => {
    const { paths, resources } = queuedPaths()
    const { store } = await storeWith({ channels: channels(), paths })
    store.dispatch(openChannelCreation())
    resources.push(
      Array.from({ length: channelRules.maxResources + 2 }, (_, n) => `/r/${n}`),
    )
    await store.dispatch(addChannelResources({ kind: "files" }))
    expect(flowOf(store)?.draft.resources).toHaveLength(channelRules.maxResources)
    expect(flowOf(store)?.leftOut).toBe(2)
  })

  it("takes files and folders dropped while it asks for them, each with its kind", async () => {
    const { paths, watches } = queuedPaths()
    const { store } = await storeWith({ channels: channels(), paths })
    store.dispatch(openChannelCreation())
    const over: boolean[] = []
    const stop = store.dispatch(watchChannelDrops({ dragging: (is) => over.push(is) }))
    let drops = 0
    const land = () => {
      const name = `d${++drops}`
      watches.forEach((watch) => watch.began(name))
      return name
    }
    const answer = (name: string, ...dropped: DroppedPath[]) =>
      watches.forEach((watch) => watch.dropped(name, dropped, 0))
    watches.forEach((watch) => watch.dragging(true))
    expect(over).toEqual([true])
    // Landed while it asked for the name: its paths add nothing, even once
    // they come with the files question open.
    const early = land()
    store.dispatch(askChannelStep({ step: "resources" }))
    answer(early, { path: "/work/early.md", kind: "file" })
    expect(flowOf(store)?.draft.resources).toEqual([])
    answer(
      land(),
      { path: "/work/brief.md", kind: "file" },
      { path: "/work/docs", kind: "folder" },
      { path: "/work/brief.md", kind: "file" },
    )
    expect(flowOf(store)?.draft.resources).toEqual(["/work/brief.md", "/work/docs"])
    expect(flowOf(store)?.kinds).toEqual({
      "/work/brief.md": "file",
      "/work/docs": "folder",
    })
    // Landed here, then the question moved on before the paths came: they
    // add nothing, and the drop no longer holds Create.
    const moved = land()
    expect(flowOf(store)?.landing).toEqual([moved])
    store.dispatch(askChannelStep({ step: "notes" }))
    answer(moved, { path: "/work/moved.md", kind: "file" })
    expect(flowOf(store)?.draft.resources).toEqual(["/work/brief.md", "/work/docs"])
    expect(flowOf(store)?.landing).toEqual([])
    store.dispatch(askChannelStep({ step: "resources" }))
    // Landed here, then the question moved on and came back before the paths
    // came: it asks for files again when they come, so they are added.
    const back = land()
    store.dispatch(askChannelStep({ step: "notes" }))
    store.dispatch(askChannelStep({ step: "resources" }))
    answer(back, { path: "/work/back.md", kind: "file" })
    expect(flowOf(store)?.draft.resources).toEqual([
      "/work/brief.md",
      "/work/docs",
      "/work/back.md",
    ])
    expect(flowOf(store)?.landing).toEqual([])
    // Landed here, then the flow closed and another opened before the paths
    // came: they belong to neither.
    const late = land()
    store.dispatch(closeChannelCreation())
    store.dispatch(openChannelCreation())
    store.dispatch(askChannelStep({ step: "resources" }))
    answer(late, { path: "/work/late.md", kind: "file" })
    expect(flowOf(store)?.draft.resources).toEqual([])
    stop()
    expect(watches.size).toBe(0)
  })

  it("waits to create while a drop's paths are still coming, then sends them", async () => {
    const { paths, watches } = queuedPaths()
    const source = channels()
    const { store } = await storeWith({ channels: source, paths })
    store.dispatch(openChannelCreation())
    store.dispatch(draftChannel({ change: { name: "drops" } }))
    store.dispatch(askChannelStep({ step: "resources" }))
    const stop = store.dispatch(watchChannelDrops({ dragging: () => {} }))
    watches.forEach((watch) => watch.began("d1"))
    expect(await store.dispatch(createChannel())).toBeUndefined()
    expect(flowOf(store)?.creating).toBe(false)
    watches.forEach((watch) =>
      watch.dropped("d1", [{ path: "/work/brief.md", kind: "file" }], 0),
    )
    expect(flowOf(store)?.draft.resources).toEqual(["/work/brief.md"])
    const made = await store.dispatch(createChannel())
    expect(made).toBeDefined()
    expect(listed.find((channel) => channel.id === made)?.name).toBe("drops")
    stop()
  })

  it("counts the paths the host did not look at as not fitting", async () => {
    const { paths, watches } = queuedPaths()
    const { store } = await storeWith({ channels: channels(), paths })
    store.dispatch(openChannelCreation())
    store.dispatch(askChannelStep({ step: "resources" }))
    const stop = store.dispatch(watchChannelDrops({ dragging: () => {} }))
    watches.forEach((watch) => watch.began("d1"))
    watches.forEach((watch) =>
      watch.dropped("d1", [{ path: "/work/brief.md", kind: "file" }], 7),
    )
    expect(flowOf(store)?.draft.resources).toEqual(["/work/brief.md"])
    expect(flowOf(store)?.leftOut).toBe(7)
    stop()
  })

  it("watches nothing where the window is never told of a drop", async () => {
    const { store } = await storeWith({ channels: channels() })
    store.dispatch(openChannelCreation())
    const stop = store.dispatch(watchChannelDrops({ dragging: () => {} }))
    expect(() => stop()).not.toThrow()
  })
})

describe("creating", () => {
  it("makes the channel from the draft, lists it, opens it on a new session's home, and closes", async () => {
    const source = channels()
    const { store } = await storeWith({ channels: source })
    store.dispatch(openChannelCreation())
    store.dispatch(
      draftChannel({ change: { name: " design-review ", notes: "Be brief." } }),
    )
    const made = await store.dispatch(createChannel())
    await settle()
    expect(made).toBe("sample-channel-1")
    expect(source.calls).toEqual([
      {
        requestId: expect.any(String),
        draft: expect.objectContaining({ name: " design-review ", notes: "Be brief." }),
      },
    ])
    const state = store.getState().workspace
    expect(state.channelCreation).toBeNull()
    expect(state.channels.at(-1)).toEqual(
      expect.objectContaining({ id: "sample-channel-1", name: "design-review" }),
    )
    expect(state.view.channelId).toBe("sample-channel-1")
    const draft = Object.values(state.drafts).find(
      (each) => each.channelId === "sample-channel-1",
    )
    // A new session in it starts on the channel's model.
    expect(draft?.model).toEqual(defaultModel())
  })

  it("asks nothing without a name", async () => {
    const source = channels()
    const { store } = await storeWith({ channels: source })
    store.dispatch(openChannelCreation())
    expect(await store.dispatch(createChannel())).toBeUndefined()
    expect(source.calls).toEqual([])
  })

  it("says a refusal at its step, and sends the same draft again under the same id", async () => {
    const { paths, folders } = queuedPaths()
    const source = channels()
    const { store } = await storeWith({ channels: source, paths })
    store.dispatch(openChannelCreation())
    store.dispatch(draftChannel({ change: { name: "a" } }))
    for (let n = 0; n < 6; n++) store.dispatch(answerChannelStep())
    folders.push("/")
    await store.dispatch(chooseChannelFolder())
    source.refuse("folder-refused")
    expect(await store.dispatch(createChannel())).toBeUndefined()
    expect(flowOf(store)).toEqual(
      expect.objectContaining({
        failure: "folder-refused",
        asking: "folder",
        creating: false,
      }),
    )
    expect(flowOf(store)?.answered).not.toContain("folder")
    source.refuse("unavailable")
    await store.dispatch(createChannel())
    await store.dispatch(createChannel())
    const [first, second, third] = source.calls.map((call) => call.requestId)
    expect(second).toBe(first)
    expect(third).toBe(first)
    expect(flowOf(store)).toBeNull()
  })

  it("takes a fresh id once the draft changes after a refusal", async () => {
    const source = channels()
    const { store } = await storeWith({ channels: source })
    store.dispatch(openChannelCreation())
    store.dispatch(draftChannel({ change: { name: "a" } }))
    source.refuse("unavailable")
    await store.dispatch(createChannel())
    store.dispatch(draftChannel({ change: { name: "b" } }))
    await store.dispatch(createChannel())
    const [first, second] = source.calls.map((call) => call.requestId)
    expect(second).not.toBe(first)
  })

  it("holds the draft while a Create is on its way", async () => {
    let answer: (value: never) => void = () => {}
    const held: ChannelSource = {
      create: () => new Promise((resolve) => (answer = resolve as never)),
    }
    const { store } = await storeWith({ channels: held })
    store.dispatch(openChannelCreation())
    store.dispatch(draftChannel({ change: { name: "a" } }))
    const creating = store.dispatch(createChannel())
    store.dispatch(draftChannel({ change: { name: "b" } }))
    expect(flowOf(store)?.draft.name).toBe("a")
    expect(flowOf(store)?.creating).toBe(true)
    answer({
      id: "made",
      name: "a",
      sectionId: "s1",
      private: false,
      topic: "",
    } as never)
    await creating
    expect(store.getState().workspace.channels.some((each) => each.id === "made")).toBe(
      true,
    )
  })

  it("lists a channel made after the flow closed, and opens nothing", async () => {
    let answer: (value: never) => void = () => {}
    const held: ChannelSource = {
      create: () => new Promise((resolve) => (answer = resolve as never)),
    }
    const { store } = await storeWith({ channels: held })
    const viewed = store.getState().workspace.view.channelId
    store.dispatch(openChannelCreation())
    store.dispatch(draftChannel({ change: { name: "a" } }))
    const creating = store.dispatch(createChannel())
    store.dispatch(closeChannelCreation())
    answer({ id: "made", name: "a", sectionId: "s1", private: false, topic: "" } as never)
    await creating
    const state = store.getState().workspace
    expect(state.channels.some((each) => each.id === "made")).toBe(true)
    expect(state.view.channelId).toBe(viewed)
  })

  it("takes an untyped failure as unavailable", async () => {
    const thrown: ChannelSource = { create: () => Promise.reject(new Error("boom")) }
    const { store } = await storeWith({ channels: thrown })
    store.dispatch(openChannelCreation())
    store.dispatch(draftChannel({ change: { name: "a" } }))
    const error = console.error
    console.error = () => {}
    try {
      await store.dispatch(createChannel())
    } finally {
      console.error = error
    }
    expect(flowOf(store)?.failure).toBe("unavailable")
    expect(new ChannelCreationError("limit").reason).toBe("limit")
  })
})
