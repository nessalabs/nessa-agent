import {
  IMAGE_ATTACHMENT_TYPES,
  linkedFileProblem,
  MAX_FILE_PATH_BYTES as MAX_FILE_PATH_BYTES_WIRE,
  MAX_IMAGE_ATTACHMENT_BYTES,
  MAX_MESSAGE_FILES,
  MAX_MESSAGE_IMAGE_BYTES,
  MAX_MESSAGE_IMAGES,
  MAX_UPLOAD_BYTES,
  ConversationErrorCode,
  NessaAttachmentError,
  NessaConversationControlError,
  NessaConversationMutationError,
  NessaRpcError,
  type NessaClient,
  type ConversationView,
} from "@nessa/client"
import { expect, it, vi } from "vitest"
import {
  AttachmentStagingError,
  ControlFailedError,
  ConversationReadFailedError,
  SubmissionRefusedError,
} from "../../application/ports"
import {
  linkablePath,
  MAX_ATTACHMENT_BYTES,
  MAX_FILE_PATH_BYTES,
  MAX_SEND_FILES,
  MAX_SEND_IMAGES,
  MAX_SEND_TOTAL_IMAGE_BYTES,
  STORED_IMAGE_TYPES,
} from "../../model"
import { BUSY_RETRY_DELAYS_MS, gatewayEffects } from "./effects"

function gatewayView(): ConversationView {
  return {
    conversationId: "server",
    revision: "capabilities",
    messages: [],
    pending: [],
    permissions: [],
    tools: [],
    capabilities: {
      queue: true,
      steer: true,
      resume: true,
      permissions: true,
      imageInput: false,
      agentFeatures: {
        permissionDenial: "supported_for_offered_permission_reviews",
        nativeHookSuppression: "unknown",
        compactionReporting: "unsupported_not_implemented",
        modelSwitchReporting: "unsupported_not_implemented",
        permissionDeferral: "unsupported_not_implemented",
        elicitationForwarding: "unknown",
        preToolPolicy: "unsupported_not_implemented",
        policyEndTurn: "unsupported_not_implemented",
        policyCloseSession: "unsupported_not_implemented",
        incomingElicitation: "unsupported_not_implemented",
      },
    },
    lifecycle: { phase: "attached" },
    truncated: false,
    queueComplete: true,
  }
}

/** A backoff nothing in the test should reach: waiting here is the failure. */
const unexpectedWait = () => Promise.reject(new Error("no wait expected"))
const effectsOf = (client: () => NessaClient | null) =>
  gatewayEffects(client, unexpectedWait)
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
it("joins concurrent creation and forwards exact stable submission IDs", async () => {
  const gate = deferred<{ conversationId: string }>()
  const create = vi.fn(() => gate.promise)
  const send = vi.fn(async () => ({
    executionId: "execution",
    requestId: "action",
    disposition: "queued" as const,
  }))
  const effects = effectsOf(
    () => ({ conversation: { create, send } }) as unknown as NessaClient,
  )
  const first = effects.create("server")
  const second = effects.create("server")
  // The agent setup chose is asked for before the creation goes out, so the
  // call lands a turn later; joining it does not wait for that.
  expect(first).toBe(second)
  await Promise.resolve()
  expect(create).toHaveBeenCalledOnce()
  gate.resolve({ conversationId: "server" })
  await Promise.all([first, second])
  await effects.send({
    conversationId: "server",
    executionId: "execution",
    actionId: "action",
    text: "exact",
    attachments: [],
    files: [],
  })
  expect(send).toHaveBeenCalledWith("server", "exact", [], [], {
    executionId: "execution",
    requestId: "action",
  })
})
it("serializes opaque-revision reads, including overlapping manual refreshes", async () => {
  const first = deferred<ConversationView>()
  const read = vi.fn(() => first.promise)
  const effects = effectsOf(() => ({ conversation: { read } }) as unknown as NessaClient)
  const pending = effects.read("server")
  const following = effects.read("server")
  await Promise.resolve()
  await Promise.resolve()
  expect(read).toHaveBeenCalledOnce()
  first.resolve(gatewayView())
  await Promise.all([pending, following])
  expect(read).toHaveBeenCalledTimes(2)
})

it("asks the host again after a failed answer instead of keeping the failure", async () => {
  // Every conversation is created through this, so a remembered rejection is
  // not one lost answer — it is a panel that can no longer start, send, close
  // or answer a permission until it is restarted.
  const create = vi.fn(async ({ conversationId }: { conversationId: string }) => ({
    conversationId,
  }))
  let asked = 0
  const chosenAgent = async () => {
    asked += 1
    if (asked === 1) throw new Error("the host module would not load")
    return "codex"
  }
  const effects = gatewayEffects(
    () => ({ conversation: { create } }) as unknown as NessaClient,
    unexpectedWait,
    chosenAgent,
  )
  await expect(effects.create("server")).rejects.toThrow("the host module would not load")
  expect(create).not.toHaveBeenCalled()
  await effects.create("server")
  expect(asked).toBe(2)
  expect(create).toHaveBeenCalledWith({ conversationId: "server", agent: "codex" })
})

it("sends the creation over the session of the moment, not the one checked first", async () => {
  // Asking the host is a round trip, and a session retired inside it must not
  // be the one this create goes out on.
  const retired = vi.fn()
  const live = vi.fn(async ({ conversationId }: { conversationId: string }) => ({
    conversationId,
  }))
  let current = retired
  const effects = gatewayEffects(
    () => ({ conversation: { create: current } }) as unknown as NessaClient,
    unexpectedWait,
    async () => {
      current = live
      return undefined
    },
  )
  await effects.create("server")
  expect(retired).not.toHaveBeenCalled()
  expect(live).toHaveBeenCalledOnce()
})

const reading = (error: unknown) =>
  effectsOf(
    () =>
      ({ conversation: { read: () => Promise.reject(error) } }) as unknown as NessaClient,
  )
    .read("server")
    .catch((error: unknown) => error)

it.each([
  // The gateway lost the configuration this conversation was created against:
  // the one read failure this panel has a separate word for.
  ["conversation_configuration_changed", "configuration-changed"],
  // And the gateway not being able to read what it saved, which it caches and
  // answers every later read from — so there is nothing left to keep trying.
  ["conversation_state_unreadable", "state-unreadable"],
  // Known codes with nothing extra to tell somebody watching a stale transcript.
  ["conversation_not_found", "unavailable"],
  ["agent_not_configured", "unavailable"],
  ["conversation_storage_unavailable", "unavailable"],
  ["conversation_capacity", "unavailable"],
  ["audit_unavailable", "unavailable"],
  ["invalid_request", "unavailable"],
] as const)(
  "turns the gateway's failed read %s into the panel's own word",
  async (code, reason) => {
    // The gateway sends the code as the message too, which is exactly the
    // coincidence nothing may depend on: the message here names another code
    // entirely and only the typed one is read.
    const error = await reading(
      new NessaRpcError(code, "conversation_configuration_changed"),
    )
    expect(error).toBeInstanceOf(ConversationReadFailedError)
    expect(error).toMatchObject({ reason })
  },
)

it.each([
  ConversationErrorCode.TemporarilyUnavailable,
  ConversationErrorCode.AgentStartupDeadline,
] as const)(
  "promises no recovery for %s, which the gateway also sends for a blocked conversation",
  async (code) => {
    // Both read as "not yet" and usually are. But `ConversationService` retains
    // a conversation's slot when a failed launch could not be confirmed stopped
    // — `a_startup_deadline_with_unconfirmed_cleanup_retains_its_slot` asserts
    // the provider is never attempted again — and an adapter panic during
    // opening reaches `temporarily_unavailable` the same way. The gateway
    // cannot tell the two apart in the code it sends, so neither may this.
    const error = await reading(new NessaRpcError(code, code))
    expect(error).toMatchObject({ reason: "unavailable" })
  },
)

it("does not read a code this build has never heard of as one it has", async () => {
  for (const cause of [
    // A code from a newer gateway, whose message is a code the panel *does*
    // have a word for: nothing about a read is decided by a string.
    new NessaRpcError("quantum_flux", "conversation_configuration_changed"),
    new NessaRpcError("", "temporarily_unavailable"),
    // The reason the wire's code is narrowed before it is looked up, rather
    // than indexed with as it arrived: these are members of every object, and
    // the values behind them are truthy and are not reasons.
    new NessaRpcError("constructor", "constructor"),
    new NessaRpcError("toString", "toString"),
  ]) {
    const error = await reading(cause)
    expect(error).toBeInstanceOf(ConversationReadFailedError)
    expect(error).toMatchObject({ reason: "unavailable", cause })
  }
})

it("gives a read with no wire answer at all the same honest word", async () => {
  // A dropped connection, a request that timed out, a view the client would not
  // validate, and no session to ask: none of them is a view, and none of them
  // claims more than that.
  for (const cause of [
    new Error("connection closed"),
    new TypeError("Conversation view is malformed"),
  ]) {
    expect(await reading(cause)).toMatchObject({ reason: "unavailable", cause })
  }
  const offline = await effectsOf(() => null)
    .read("server")
    .catch((error: unknown) => error)
  expect(offline).toBeInstanceOf(ConversationReadFailedError)
  expect(offline).toMatchObject({ reason: "unavailable" })
})

it("keeps a failed read from settling the next one, and translates each on its own", async () => {
  // Reads are serialized through one chain. A rejection must not travel down it
  // and answer for a request that was never made.
  const read = vi
    .fn()
    .mockRejectedValueOnce(
      new NessaRpcError("conversation_configuration_changed", "setup changed"),
    )
    .mockResolvedValueOnce(gatewayView())
  const effects = effectsOf(() => ({ conversation: { read } }) as unknown as NessaClient)
  const failed = effects.read("server").catch((error: unknown) => error)
  const following = effects.read("server")
  expect(await failed).toMatchObject({ reason: "configuration-changed" })
  expect(await following).toMatchObject({
    conversationId: "server",
    capabilities: {
      agentFeatures: {
        permissionDenial: "supported_for_offered_permission_reviews",
        preToolPolicy: "unsupported_not_implemented",
      },
    },
  })
  expect(read).toHaveBeenCalledTimes(2)
})

/** The original bytes, described as they are uploaded. */
const file = { digest: `sha256:${"ab".repeat(32)}`, mimeType: "image/heic", size: 3 }
/** What the gateway stored them as. */
const stored = {
  digest: `sha256:${"ef".repeat(32)}`,
  mimeType: "image/jpeg" as const,
  size: 2,
}
const ticket = "cd".repeat(32)
const bytes = new Blob(["raw"], { type: "image/heic" })
/** A signal nobody aborts. */
const live = () => new AbortController().signal
const owed = { requestId: "r", state: "upload_required", ticket, expiresAtMs: 1 }
function staging(
  attachments: { begin: unknown; upload?: unknown },
  wait: (ms: number) => Promise<void> = unexpectedWait,
) {
  return gatewayEffects(
    () =>
      ({ attachments: { upload: vi.fn(), ...attachments } }) as unknown as NessaClient,
    wait,
  )
}

it("holds every message bound the model keeps to the one the protocol generated", () => {
  // Two sets of constants on purpose — the conversation model does not import a
  // client SDK — and this adapter, which sees both, is where they are held to
  // each other. The client's side of each is generated from
  // `protocol/product/v1.json`, so a schema change that nobody carried into the
  // model fails here rather than at the gateway.
  expect([...STORED_IMAGE_TYPES]).toEqual([...IMAGE_ATTACHMENT_TYPES])
  expect(MAX_SEND_IMAGES).toBe(MAX_MESSAGE_IMAGES)
  expect(MAX_SEND_TOTAL_IMAGE_BYTES).toBe(MAX_MESSAGE_IMAGE_BYTES)
  // What one file may weigh to be attached at all is the upload path's bound.
  expect(MAX_ATTACHMENT_BYTES).toBe(MAX_UPLOAD_BYTES)
  expect(MAX_SEND_FILES).toBe(MAX_MESSAGE_FILES)
  expect(MAX_FILE_PATH_BYTES).toBe(MAX_FILE_PATH_BYTES_WIRE)
})

it("refuses locally exactly the paths the published rule refuses", () => {
  // The model states the path rule in its own terms, because it does not
  // import a client SDK, and the client compiles the one the schema publishes.
  // Two statements of one rule is how the client came to accept a C1 control
  // the gateway refused — and a message refused as `invalid_request` shows no
  // sentence of its own, so that gap said nothing at all. This is where they
  // are held to each other.
  for (const path of [
    "/a",
    "/Users/ada/report.pdf",
    "/Users/ada/report (final) 100%.pdf",
    "/Users/ada/2026-09-20 10:30.txt",
    "/Users/ada/отчёт.pdf",
    "/Users/ada/.zshrc",
    "/Users/ada/...",
    "",
    "report.pdf",
    "./report.pdf",
    "/Users/ada/a]b.pdf",
    "/Users/ada/[x/report.pdf",
    "/Users/ada/a\nb.pdf",
    "/Users/ada/a\u0000b.pdf",
    "/Users/ada/a\u0085b.pdf",
    "/Users/ada/a\u009fb.pdf",
    "/Users/ada/a\u007fb.pdf",
    "/",
    "/Users/ada/",
    "//Users/ada/report.pdf",
    "/Users//ada/report.pdf",
    "/Users/ada/..",
    "/Users/../etc/passwd",
    "/Users/./ada/report.pdf",
    `/${"a".repeat(4096)}`,
    `/${"a".repeat(4095)}`,
  ])
    expect([path, linkablePath(path)]).toEqual([
      path,
      linkedFileProblem({ path }) === undefined,
    ])
})

it("forwards a message's images and files with its text, for send and steer alike", async () => {
  const receipt = { executionId: "execution", requestId: "action", disposition: "queued" }
  const send = vi.fn(async () => receipt)
  const steer = vi.fn(async () => receipt)
  const effects = effectsOf(
    () => ({ conversation: { send, steer } }) as unknown as NessaClient,
  )
  const linked = { path: "/Users/ada/report.pdf" }
  const submission = {
    conversationId: "server",
    executionId: "execution",
    actionId: "action",
    text: "",
    attachments: [stored],
    files: [linked],
  }
  await effects.send(submission)
  await effects.steer(submission)
  const ids = { executionId: "execution", requestId: "action" }
  // The two lists stay apart all the way to the client: an image's bytes were
  // carried here and a file's path was not.
  expect(send).toHaveBeenCalledExactlyOnceWith("server", "", [stored], [linked], ids)
  expect(steer).toHaveBeenCalledExactlyOnceWith("server", "", [stored], [linked], ids)
})

it.each([
  ["image_input_unsupported", "image-input-unsupported"],
  ["attachment_not_found", "attachment-not-found"],
  ["attachment_unavailable", "attachment-unavailable"],
  ["conversation_not_found", "conversation-not-found"],
  ["conversation_capacity", "conversation-capacity"],
  ["agent_not_configured", "agent-not-configured"],
  ["agent_startup_deadline", "agent-startup-deadline"],
  ["invalid_request", "invalid-request"],
] as const)(
  "turns the gateway's pre-admission refusal %s into a typed refusal, for send and steer",
  async (code, reason) => {
    const refuse = (conversationId: string) =>
      Promise.reject(
        new NessaConversationMutationError(
          conversationId,
          "action",
          "execution",
          // The message names another code: only the typed one is read.
          new NessaRpcError(code, "temporarily_unavailable"),
          () => Promise.reject(new Error("unused")),
        ),
      )
    const effects = effectsOf(
      () => ({ conversation: { send: refuse, steer: refuse } }) as unknown as NessaClient,
    )
    const submission = {
      conversationId: "server",
      executionId: "execution",
      actionId: "action",
      text: "look",
      attachments: [stored],
      files: [],
    }
    for (const submit of [effects.send, effects.steer]) {
      const error = await submit(submission).catch((error: unknown) => error)
      expect(error).toBeInstanceOf(SubmissionRefusedError)
      expect(error).toMatchObject({ reason })
    }
  },
)

it("reports the client refusing a message's images as a certain refusal, for send and steer", async () => {
  // The client validates a message's images before anything reaches the wire —
  // one boundary — and answers a bad argument with a TypeError. Nothing was
  // sent, so this is as certain as a refusal gets: not "delivery unknown".
  const refuse = () => {
    throw new TypeError(
      `Invalid message attachments: an image must contain 1-${MAX_IMAGE_ATTACHMENT_BYTES} bytes`,
    )
  }
  const effects = effectsOf(
    () => ({ conversation: { send: refuse, steer: refuse } }) as unknown as NessaClient,
  )
  for (const submit of [effects.send, effects.steer]) {
    const error = await submit({
      conversationId: "server",
      executionId: "execution",
      actionId: "action",
      text: "look",
      attachments: [{ ...stored, size: MAX_IMAGE_ATTACHMENT_BYTES + 1 }],
      files: [],
    }).catch((error: unknown) => error)
    expect(error).toBeInstanceOf(SubmissionRefusedError)
    expect(error).toMatchObject({ reason: "invalid-request" })
    // The client's sentence names what is wrong with the message; keep it.
    expect((error as Error).message).toMatch(/an image must contain/)
  }
})

it("passes an uncertain send failure on untouched: a lost answer is not a refusal", async () => {
  const lost = new NessaConversationMutationError(
    "server",
    "action",
    "execution",
    new NessaRpcError("temporarily_unavailable", "attachment_not_found"),
    () => Promise.reject(new Error("unused")),
  )
  const effects = effectsOf(
    () =>
      ({ conversation: { send: () => Promise.reject(lost) } }) as unknown as NessaClient,
  )
  const error = await effects
    .send({
      conversationId: "server",
      executionId: "execution",
      actionId: "action",
      text: "look",
      attachments: [],
      files: [],
    })
    .catch((error: unknown) => error)
  expect(error).toBe(lost)
})

const controlError = (code: string) =>
  new NessaConversationControlError(
    "server",
    "action",
    undefined,
    // The message names another code: only the typed one is read.
    new NessaRpcError(code, "temporarily_unavailable"),
  )

/** A permission answer the gateway failed, reporting what became of the review. */
const answerError = (code: string, selectionState: string) =>
  new NessaConversationControlError(
    "server",
    "action",
    "execution",
    new NessaRpcError(code, "temporarily_unavailable", { selectionState }),
    true,
  )

/** What the client says about a control it refused, by wire code.
 *
 * A control resolves its conversation before anything is dispatched, so it
 * meets exactly the refusals a creation meets and meets them just as early:
 * `conversation_not_found` is therefore `refused` — the gateway does not have
 * this conversation, so nothing was done — rather than an outcome nobody can
 * describe. `attachment_cleanup_unavailable` is the one that genuinely stays
 * unknown, because the close itself may already have applied.
 *
 * The message is the refusal's own sentence where the client has one, and the
 * general one where it does not: `invalid_request` has nothing to add beyond
 * what the sentence already says, and a cleanup failure is not a refusal at
 * all.
 */
const CONTROL_REFUSED =
  "Conversation control did not return a trustworthy acknowledgement"

it.each([
  [
    "attachment_cleanup_unavailable",
    "attachment-cleanup-unavailable",
    "unknown",
    CONTROL_REFUSED,
  ],
  [
    "agent_startup_deadline",
    "agent-startup-deadline",
    "refused",
    "The agent was still starting and ran out of time, so nothing was sent. Starting it is slowest the first time after an install or update, while the operating system scans the runtime. Retry normally succeeds once the runtime is warm.",
  ],
  ["conversation_not_found", "conversation-not-found", "refused", CONTROL_REFUSED],
  ["invalid_request", "invalid-request", "refused", CONTROL_REFUSED],
] as const)(
  "turns the gateway's control failure %s into the panel's own word for it, with its outcome",
  async (code, reason, outcome, message) => {
    const refuse = () => Promise.reject(controlError(code))
    const effects = effectsOf(
      () =>
        ({
          conversation: {
            close: refuse,
            remove: refuse,
            answer: refuse,
            cancel: refuse,
            reorder: refuse,
          },
        }) as unknown as NessaClient,
    )
    const controls = [
      () => effects.close("server"),
      () => effects.remove("server", "execution"),
      () => effects.answer("server", "execution", "permission", "option"),
      () => effects.cancel("server", "execution", "permission"),
      () => effects.reorder("server", ["execution"]),
    ]
    for (const control of controls) {
      const error = await control().catch((error: unknown) => error)
      expect(error).toBeInstanceOf(ControlFailedError)
      // The reason and the outcome, as two facts: the same reason can arrive
      // either way, so neither may be read out of the other.
      expect(error).toMatchObject({ reason, outcome })
      // A control the gateway refused says why, in the client's own words,
      // where the client has a sentence for that refusal. Where it has none,
      // the general text stands; what the panel does with either is the
      // application's to decide.
      expect((error as Error).message).toBe(message)
    }
  },
)

it("keeps a cleanup failure's reason even though the close itself may have applied", async () => {
  // `attachment_cleanup_unavailable` is the one image code that is not a
  // refusal: the close happened and only its release of the uploads did not,
  // so the client leaves it uncertain. The reason still has to reach the panel
  // — which is why a control is translated regardless of that verdict.
  const uncertain = controlError("attachment_cleanup_unavailable")
  expect(uncertain.uncertain).toBe(true)
  const effects = effectsOf(
    () =>
      ({
        conversation: { close: () => Promise.reject(uncertain) },
      }) as unknown as NessaClient,
  )
  const error = await effects.close("server").catch((error: unknown) => error)
  expect(error).toMatchObject({
    reason: "attachment-cleanup-unavailable",
    outcome: "unknown",
  })
})

const answering = (error: unknown) =>
  effectsOf(
    () =>
      ({
        conversation: { answer: () => Promise.reject(error) },
      }) as unknown as NessaClient,
  ).answer("server", "execution", "permission", "option")

it.each([
  // The gateway sends an ordinary diagnostic code beside a pending review; the
  // server's own test pairs it with `audit_unavailable`. Two of these have no
  // word here at all, and the review's state is certain in every one of them.
  ["audit_unavailable", undefined],
  ["stale_permission", undefined],
  ["conversation_not_found", "conversation-not-found"],
] as const)(
  "reports a review the gateway left pending as refused, under %s",
  async (code, reason) => {
    const pending = answerError(code, "pending")
    expect(pending.uncertain).toBe(false)
    const error = await answering(pending).catch((error: unknown) => error)
    expect(error).toBeInstanceOf(ControlFailedError)
    // The reason may be unknown; what became of the review is not, and a code
    // with no word for it must not take the outcome down with it.
    expect(error).toMatchObject({ reason, outcome: "refused" })
  },
)

it("reports a review the gateway consumed as applied, not as an untrustworthy answer", async () => {
  // The protocol calls the selection state authoritative and independent of the
  // code, so a consumed option means the choice took effect however the rest of
  // the command ended. The client can only call that uncertain.
  const consumed = answerError("audit_unavailable", "consumed")
  expect(consumed.uncertain).toBe(true)
  const error = await answering(consumed).catch((error: unknown) => error)
  expect(error).toMatchObject({ reason: undefined, outcome: "applied" })
})

it("leaves a review whose state the gateway could not prove genuinely unknown", async () => {
  const unproven = answerError("audit_unavailable", "unknown")
  // No word for the code and nothing certain about the review: there is nothing
  // this panel can add to what the client already says, so it says nothing.
  expect(await answering(unproven).catch((error: unknown) => error)).toBe(unproven)
})

it("gives every code the client decides before admission a word of its own", async () => {
  // The store no longer reads the client's `uncertain` for a message: it takes
  // a typed refusal to mean the draft comes back, and nothing else. That rests
  // on this adapter answering with one for every code the client decides that
  // way, so the two sets are checked against each other rather than assumed.
  // A code added to the client's pre-admission list without a word here fails.
  for (const code of Object.values(ConversationErrorCode)) {
    const rejection = new NessaConversationMutationError(
      "server",
      "action",
      "execution",
      new NessaRpcError(code, code),
      () => Promise.reject(new Error("unused")),
    )
    if (rejection.uncertain) continue
    const effects = effectsOf(
      () =>
        ({
          conversation: { send: () => Promise.reject(rejection) },
        }) as unknown as NessaClient,
    )
    const error = await effects
      .send({
        conversationId: "server",
        executionId: "execution",
        actionId: "action",
        text: "look",
        attachments: [],
        files: [],
      })
      .catch((error: unknown) => error)
    expect(
      error,
      `${code} is refused before admission with no word for it`,
    ).toBeInstanceOf(SubmissionRefusedError)
  }
})

it("passes a control failure this build has no word for on untouched", async () => {
  const unknown = controlError("quantum_flux")
  expect(unknown.code).toBeUndefined()
  const effects = effectsOf(
    () =>
      ({
        conversation: { close: () => Promise.reject(unknown) },
      }) as unknown as NessaClient,
  )
  expect(await effects.close("server").catch((error: unknown) => error)).toBe(unknown)
})

it("refuses an unopenable conversation in the panel's words, once, for every waiter", async () => {
  // `create` is the prerequisite of both a send and a control, and is joined
  // across callers; every one of them gets the same translated refusal.
  const create = vi.fn(() =>
    Promise.reject(
      new NessaConversationMutationError(
        "server",
        "action",
        undefined,
        new NessaRpcError("agent_startup_deadline", "agent_startup_deadline"),
        () => Promise.reject(new Error("unused")),
      ),
    ),
  )
  const effects = effectsOf(
    () => ({ conversation: { create } }) as unknown as NessaClient,
  )
  const failures = await Promise.all(
    [effects.create("server"), effects.create("server")].map((pending) =>
      pending.catch((error: unknown) => error),
    ),
  )
  expect(create).toHaveBeenCalledOnce()
  for (const error of failures) {
    expect(error).toBeInstanceOf(SubmissionRefusedError)
    expect(error).toMatchObject({ reason: "agent-startup-deadline" })
    // The client's long sentence about a cold runtime, not a word from here.
    expect((error as Error).message).toMatch(/still starting and ran out of time/)
  }
})

it("uploads nothing when the conversation already holds the bytes, and answers with its reference", async () => {
  const begin = vi.fn(async () => ({ requestId: "r", state: "stored", stored }))
  const upload = vi.fn()
  expect(
    await staging({ begin, upload }).stageAttachment("server", file, bytes, live()),
  ).toEqual(stored)
  expect(begin).toHaveBeenCalledExactlyOnceWith("server", file)
  expect(upload).not.toHaveBeenCalled()
})

it("uploads the original bytes under the ticket and answers with what was stored", async () => {
  const begin = vi.fn(async () => owed)
  const upload = vi.fn(async () => stored)
  const reference = await staging({ begin, upload }).stageAttachment(
    "server",
    file,
    bytes,
    live(),
  )
  expect(upload).toHaveBeenCalledExactlyOnceWith(
    ticket,
    { mimeType: "image/heic", bytes },
    { signal: expect.any(AbortSignal) },
  )
  // The gateway's reference, not a restatement of what was uploaded.
  expect(reference).toEqual(stored)
  expect(reference.digest).not.toBe(file.digest)
})

it.each([
  ["a file that is not an image", { mimeType: "application/pdf" }, "unsupported-image"],
  [
    "an image the gateway left in an encoding no message names",
    { mimeType: "image/heic" },
    "unsupported-image",
  ],
  // Two different facts, and the tile offers a retry for neither — but it says
  // something different for each, so a readable 6 MiB PNG is not reported as a
  // format the gateway could not read.
  [
    "a readable image over the protocol's image bound",
    { size: MAX_IMAGE_ATTACHMENT_BYTES + 1 },
    "too-large",
  ],
  // Neither fact: the gateway answered something this window cannot use at all.
  ["a reference that is malformed", { digest: "sha256:NOPE" }, "rejected"],
] as const)(
  "does not hand a message %s, however validly it was stored",
  async (_name, change, reason) => {
    const kept = { ...stored, ...change }
    for (const attachments of [
      { begin: async () => ({ requestId: "r", state: "stored", stored: kept }) },
      { begin: async () => owed, upload: async () => kept },
    ])
      await expect(
        staging(attachments).stageAttachment("server", file, bytes, live()),
      ).rejects.toMatchObject({ reason })
  },
)

it("keeps an image at exactly the protocol's bound", async () => {
  const kept = { ...stored, size: MAX_IMAGE_ATTACHMENT_BYTES }
  expect(
    await staging({ begin: async () => owed, upload: async () => kept }).stageAttachment(
      "server",
      file,
      bytes,
      live(),
    ),
  ).toEqual(kept)
})

it.each([
  ["unsupported_image", "unsupported-image"],
  ["image_too_large", "too-large"],
  ["image_input_unsupported", "image-input-unsupported"],
  ["upload_interrupted", "interrupted"],
  ["attachment_not_kept", "interrupted"],
  ["upload_unresolved", "interrupted"],
  ["upload_timeout", "interrupted"],
  ["aborted", "interrupted"],
  ["ticket_invalid", "unavailable"],
  ["storage_unavailable", "unavailable"],
  ["audit_unavailable", "unavailable"],
  ["unreachable", "unavailable"],
  ["size_mismatch", "rejected"],
  ["digest_mismatch", "rejected"],
  ["unexpected_response", "rejected"],
] as const)("maps an upload that failed as %s to %s", async (code, reason) => {
  const cause = new NessaAttachmentError(code, 400)
  const upload = vi.fn(() => Promise.reject(cause))
  const error = await staging({ begin: async () => owed, upload })
    .stageAttachment("server", file, bytes, live())
    .catch((error: unknown) => error)
  expect(error).toBeInstanceOf(AttachmentStagingError)
  expect(error).toMatchObject({ reason, cause })
  // Every one of these spends the ticket or leaves it unknown: no second PUT.
  expect(upload).toHaveBeenCalledOnce()
})

/** A backoff the test ends by hand: each wait is a promise it resolves. */
function manualWait() {
  const waits: { ms: number; done: () => void }[] = []
  const asked = { next: () => {} }
  const wait = (ms: number) =>
    new Promise<void>((done) => {
      waits.push({ ms, done })
      asked.next()
    })
  /** Resolves once the adapter is waiting for the `count`th time. */
  const reached = (count: number) =>
    new Promise<void>((resolve) => {
      const check = () => (waits.length >= count ? resolve() : undefined)
      asked.next = check
      check()
    })
  return { wait, waits, reached }
}
const busy = () => new NessaAttachmentError("temporarily_unavailable", 503)

it("offers the same ticket again after a short wait when the gateway had no room", async () => {
  const clock = manualWait()
  const begin = vi.fn(async () => owed)
  const upload = vi
    .fn()
    .mockRejectedValueOnce(busy())
    .mockRejectedValueOnce(busy())
    .mockResolvedValueOnce(stored)
  const staged = staging({ begin, upload }, clock.wait).stageAttachment(
    "server",
    file,
    bytes,
    live(),
  )
  await clock.reached(1)
  // Nothing is sent again until the wait is over.
  expect(upload).toHaveBeenCalledOnce()
  clock.waits[0]!.done()
  await clock.reached(2)
  expect(upload).toHaveBeenCalledTimes(2)
  clock.waits[1]!.done()
  expect(await staged).toEqual(stored)
  expect(clock.waits.map((wait) => wait.ms)).toEqual(BUSY_RETRY_DELAYS_MS.slice(0, 2))
  // One begin, one ticket: that refusal is the one that does not spend it.
  expect(begin).toHaveBeenCalledOnce()
  expect(upload.mock.calls.map(([used]) => used)).toEqual([ticket, ticket, ticket])
})

it("gives up as busy after a bounded number of tries, and says so on the tile's terms", async () => {
  const clock = manualWait()
  const upload = vi.fn(() => Promise.reject(busy()))
  const staged = staging({ begin: async () => owed, upload }, clock.wait)
    .stageAttachment("server", file, bytes, live())
    .catch((error: unknown) => error)
  for (let index = 0; index < BUSY_RETRY_DELAYS_MS.length; index++) {
    await clock.reached(index + 1)
    clock.waits[index]!.done()
  }
  expect(await staged).toMatchObject({ reason: "busy" })
  expect(upload).toHaveBeenCalledTimes(BUSY_RETRY_DELAYS_MS.length + 1)
  expect(clock.waits.map((wait) => wait.ms)).toEqual([...BUSY_RETRY_DELAYS_MS])
})

it("stops offering the ticket when the session went away during the wait", async () => {
  const clock = manualWait()
  const upload = vi.fn(() => Promise.reject(busy()))
  let status = "connected"
  const effects = gatewayEffects(
    () =>
      ({
        connectionState: { status },
        attachments: { begin: async () => owed, upload },
      }) as unknown as NessaClient,
    clock.wait,
  )
  const staged = effects
    .stageAttachment("server", file, bytes, live())
    .catch((error: unknown) => error)
  await clock.reached(1)
  status = "reconnecting"
  clock.waits[0]!.done()
  expect(await staged).toMatchObject({ reason: "unavailable" })
  expect(upload).toHaveBeenCalledOnce()
})

it.each([
  ["attachment_capacity", "busy"],
  ["temporarily_unavailable", "busy"],
  ["attachment_storage_unavailable", "unavailable"],
  ["storage_unavailable", "unavailable"],
  ["audit_unavailable", "unavailable"],
  ["agent_not_configured", "unavailable"],
  ["conversation_not_found", "unavailable"],
  ["image_input_unsupported", "image-input-unsupported"],
  ["invalid_request", "rejected"],
  ["unexpected", "rejected"],
] as const)("maps a begin the gateway refused as %s to %s", async (refusal, reason) => {
  const upload = vi.fn()
  const begin = () =>
    Promise.reject(
      new NessaAttachmentError("begin_refused", undefined, undefined, refusal),
    )
  await expect(
    staging({ begin, upload }).stageAttachment("server", file, bytes, live()),
  ).rejects.toMatchObject({ reason })
  expect(upload).not.toHaveBeenCalled()
})

it("maps an unanswered begin, and one the client would not send, without reaching the upload", async () => {
  const upload = vi.fn()
  for (const [error, reason] of [
    [new NessaAttachmentError("unreachable"), "unavailable"],
    // The client refusing to describe these bytes at all; asking again changes nothing.
    [new TypeError("Media type must be lowercase, without parameters"), "rejected"],
  ] as const) {
    await expect(
      staging({ begin: () => Promise.reject(error), upload }).stageAttachment(
        "server",
        file,
        bytes,
        live(),
      ),
    ).rejects.toMatchObject({ reason })
  }
  expect(upload).not.toHaveBeenCalled()
})

it("reports staging with no live connection as unavailable, asking nothing", async () => {
  const begin = vi.fn()
  const offline = effectsOf(() => null)
  await expect(
    offline.stageAttachment("server", file, bytes, live()),
  ).rejects.toMatchObject({
    reason: "unavailable",
  })
  const reconnecting = effectsOf(
    () =>
      ({
        connectionState: { status: "reconnecting" },
        attachments: { begin },
      }) as unknown as NessaClient,
  )
  await expect(
    reconnecting.stageAttachment("server", file, bytes, live()),
  ).rejects.toBeInstanceOf(AttachmentStagingError)
  expect(begin).not.toHaveBeenCalled()
})

it("hands the caller's signal to the upload, and stops offering a busy ticket once it is aborted", async () => {
  const begin = vi.fn(async () => owed)
  const stopping = new AbortController()
  const upload = vi.fn(async () => {
    // Removed while the gateway had no room: the wait is not worth taking.
    stopping.abort()
    throw busy()
  })
  const error = await staging({ begin, upload }, unexpectedWait)
    .stageAttachment("server", file, bytes, stopping.signal)
    .catch((cause: unknown) => cause)
  expect(upload).toHaveBeenCalledExactlyOnceWith(
    ticket,
    { mimeType: file.mimeType, bytes },
    { signal: stopping.signal },
  )
  expect(error).toBeInstanceOf(AttachmentStagingError)
})
