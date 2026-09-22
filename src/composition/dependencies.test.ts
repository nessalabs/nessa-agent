import type { NessaClient, NessaClientConnectOptions } from "@nessa/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { createDependencies } from "./dependencies"
import { scenarioEffects } from "../conversation/adapters/scenario/effects"
import { makeStore } from "../store"
import { sendDraft } from "../conversation/adapters/store/slice"
import { sha256Digest } from "../panel/adapters/sha256"
import { loadEnvironment } from "../env/environment"

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock("@tauri-apps/api/core", () => ({ invoke }))

describe("application dependency scope", () => {
  it("keeps explicit gateway authority distinct from derived endpoint discovery", async () => {
    const selected: string[] = []
    const clientConnect = vi.fn(async (options: NessaClientConnectOptions) => {
      const url =
        options.url ??
        (await options.endpointSource?.load({ stage: options.stage ?? "dev" }))
      if (!url) throw new Error("missing selected endpoint")
      selected.push(url)
      await options.credentialSource?.load({
        stage: options.stage ?? "dev",
        url,
        clientId: options.client.id,
      })
      return {
        productSession: {},
        server: { health: vi.fn(async () => ({})) },
        close: vi.fn(),
      } as unknown as NessaClient
    }) as unknown as typeof NessaClient.connect

    const explicitEndpoint = { load: vi.fn(async () => "ws://127.0.0.1:9137") }
    const explicitCredential = { load: vi.fn(async () => "explicit-secret") }
    await createDependencies({
      environment: loadEnvironment({
        VITE_NESSA_GATEWAY_URL: "http://127.0.0.1:42177/path-is-an-origin-only-input",
      }),
      endpointSource: explicitEndpoint,
      credentialSource: explicitCredential,
      clientConnect,
    }).connectSession()

    expect(explicitEndpoint.load).not.toHaveBeenCalled()
    expect(explicitCredential.load).toHaveBeenCalledWith(
      expect.objectContaining({ url: "ws://127.0.0.1:42177" }),
    )

    const discoveredEndpoint = { load: vi.fn(async () => "ws://127.0.0.1:43177") }
    const discoveredCredential = { load: vi.fn(async () => "discovered-secret") }
    await createDependencies({
      environment: loadEnvironment({}),
      endpointSource: discoveredEndpoint,
      credentialSource: discoveredCredential,
      clientConnect,
    }).connectSession()

    expect(discoveredEndpoint.load).toHaveBeenCalledWith({ stage: "prod" })
    expect(discoveredCredential.load).toHaveBeenCalledWith(
      expect.objectContaining({ url: "ws://127.0.0.1:43177" }),
    )
    expect(selected).toEqual(["ws://127.0.0.1:42177", "ws://127.0.0.1:43177"])
  })

  it("routes real conversation operations through each injected adapter without sharing state", async () => {
    const first = makeStore(createDependencies({ conversation: scenarioEffects("echo") }))
    const second = makeStore(
      createDependencies({ conversation: scenarioEffects("echo") }),
    )
    await Promise.all([
      first.dispatch(sendDraft({ content: [{ type: "text", text: "first" }] })),
      second.dispatch(sendDraft({ content: [{ type: "text", text: "second" }] })),
    ])
    expect(first.getState().conversation.conversations[0]!.turns[1]).toMatchObject({
      text: "first",
    })
    expect(second.getState().conversation.conversations[0]!.turns[1]).toMatchObject({
      text: "second",
    })
    expect(first.getState().conversation.conversations[0]!.serverConversationId).not.toBe(
      second.getState().conversation.conversations[0]!.serverConversationId,
    )
    const disconnected = makeStore()
    await disconnected.dispatch(sendDraft({ content: [{ type: "text", text: "hello" }] }))
    expect(disconnected.getState().conversation.conversations[0]!.turns[0]).toMatchObject(
      { receipt: "failed" },
    )
    expect(disconnected.getState().conversation.conversations[0]!.turns).toHaveLength(1)
  })

  it("hands out the digest an upload is identified by, and takes a substitute for it", async () => {
    // Hashing reads Web Crypto, which is outside this process like a socket or
    // a clock, so it arrives from here rather than being imported where it is
    // used — and a test can hash without `crypto.subtle` anywhere near it.
    const bytes = new Blob(["abc"])
    expect(await createDependencies().digest(bytes)).toBe(await sha256Digest(bytes))
    const substitute = vi.fn(async () => `sha256:${"cd".repeat(32)}`)
    const scoped = createDependencies({ digest: substitute })
    expect(await scoped.digest(bytes)).toBe(`sha256:${"cd".repeat(32)}`)
    expect(substitute).toHaveBeenCalledExactlyOnceWith(bytes)
  })
})

describe("the agent every conversation is created on", () => {
  beforeEach(() => {
    vi.resetModules()
    invoke.mockReset()
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} })
  })
  afterEach(() => vi.unstubAllGlobals())

  it("is asked for again while nobody has chosen, because setup finishes after the panel is usable", async () => {
    // The panel exists before the choice is written down. Setup runs in a
    // second window whose summon step puts the user in this composer, one step
    // after picking an agent and before anything records it — so the host
    // answering "nobody has chosen" is the ordinary case during a first run,
    // not a failure. Remembering it sends every conversation for the rest of
    // the launch to the gateway's default, which looks exactly like the choice
    // having been honoured.
    const { createDependencies } = await import("./dependencies")
    const create = vi.fn(async ({ conversationId }: { conversationId: string }) => ({
      conversationId,
    }))
    const dependencies = createDependencies()
    dependencies.session.set({ conversation: { create } } as unknown as NessaClient)

    // The same answer `chosen_agent` gives before `finish_setup` has run, and
    // the one an unreadable `settings.json` gives too.
    invoke.mockResolvedValueOnce(null)
    await dependencies.conversation.create("during-setup")
    expect(create).toHaveBeenLastCalledWith({
      conversationId: "during-setup",
      agent: undefined,
    })

    invoke.mockResolvedValue("codex")
    await dependencies.conversation.create("after-setup")
    expect(create).toHaveBeenLastCalledWith({
      conversationId: "after-setup",
      agent: "codex",
    })
    expect(invoke).toHaveBeenCalledTimes(2)
  })

  it("is not asked at all once a surface with no host has handed its choice over", async () => {
    // A browser has nothing to write the choice to, so setup tells the
    // dependencies in place. From then on there is nothing to ask.
    const { createDependencies } = await import("./dependencies")
    const create = vi.fn(async ({ conversationId }: { conversationId: string }) => ({
      conversationId,
    }))
    const dependencies = createDependencies()
    dependencies.session.set({ conversation: { create } } as unknown as NessaClient)

    dependencies.rememberChosenAgent("codex")
    await dependencies.conversation.create("in-place")
    expect(create).toHaveBeenLastCalledWith({
      conversationId: "in-place",
      agent: "codex",
    })
    expect(invoke).not.toHaveBeenCalled()
  })

  it("keeps a choice handed over while a host read was still in flight", async () => {
    // The surface with no host to ask is exactly the one that hands the choice
    // over in place, so the two can overlap. An answer that lands afterwards
    // must not erase what it knows nothing about.
    const { createDependencies } = await import("./dependencies")
    const create = vi.fn(async ({ conversationId }: { conversationId: string }) => ({
      conversationId,
    }))
    const dependencies = createDependencies()
    dependencies.session.set({ conversation: { create } } as unknown as NessaClient)

    let answer!: (value: null) => void
    invoke.mockReturnValueOnce(
      new Promise<null>((resolve) => {
        answer = resolve
      }),
    )
    const asking = dependencies.conversation.create("during-setup")
    dependencies.rememberChosenAgent("codex")
    answer(null)
    await asking

    await dependencies.conversation.create("after-handover")
    expect(create).toHaveBeenLastCalledWith({
      conversationId: "after-handover",
      agent: "codex",
    })
    // And the host is not asked again: the choice was handed over, not read.
    expect(invoke).toHaveBeenCalledTimes(1)
  })

  it("is asked for again after a host that could not answer, not settled for good", async () => {
    // Driven through the real lookup rather than a stand-in, because the trap
    // is in the real one: it survives a failed host by resolving to nothing, so
    // a caller that remembers answers remembers "nobody chose" — and the agent
    // the user picked is never asked for again, however well the host recovers.
    const { createDependencies } = await import("./dependencies")
    const create = vi.fn(async ({ conversationId }: { conversationId: string }) => ({
      conversationId,
    }))
    const dependencies = createDependencies()
    dependencies.session.set({ conversation: { create } } as unknown as NessaClient)

    invoke.mockRejectedValueOnce(new Error("the host is not up yet"))
    await dependencies.conversation.create("first")
    expect(create).toHaveBeenLastCalledWith({
      conversationId: "first",
      agent: undefined,
    })

    // The host recovers, and the saved choice is honoured from here on.
    invoke.mockResolvedValue("codex")
    await dependencies.conversation.create("second")
    expect(create).toHaveBeenLastCalledWith({
      conversationId: "second",
      agent: "codex",
    })

    // And having been answered, it is not asked a third time.
    await dependencies.conversation.create("third")
    expect(invoke).toHaveBeenCalledTimes(2)
    expect(create).toHaveBeenLastCalledWith({
      conversationId: "third",
      agent: "codex",
    })
  })
})
