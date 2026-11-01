import { describe, expect, it, vi } from "vitest"
import type { VoiceCall } from "@nessa/client"
import {
  TransportRefused,
  VoiceStartRefused,
  type AgentReply,
  type RealtimeConnection,
  type RealtimeEvent,
  type SendResult,
  type TurnState,
  type VoiceConversation,
} from "./ports"
import { TRANSCRIPT_WAIT_MS, VoiceController } from "./voice-controller"

const grant: VoiceCall = {
  transport: "scripted",
  callUrl: "ws://127.0.0.1:1/realtime",
  clientSecret: "ek_1",
  expiresAt: 60_000,
  model: "gpt-realtime",
}

class FakeConnection implements RealtimeConnection {
  readonly sent: RealtimeEvent[] = []
  closed = false
  microphone: boolean | null = null
  speaker: boolean | null = null
  private listener: (event: RealtimeEvent) => void = () => {}
  private closedListener: () => void = () => {}
  send(event: RealtimeEvent) {
    this.sent.push(event)
  }
  onEvent(listener: (event: RealtimeEvent) => void) {
    this.listener = listener
  }
  onClosed(listener: () => void) {
    this.closedListener = listener
  }
  levels() {
    return { input: 0.5, output: 0.25 }
  }
  setMicrophone(on: boolean) {
    this.microphone = on
  }
  setSpeaker(on: boolean) {
    this.speaker = on
  }
  close() {
    this.closed = true
  }
  /** The provider sends `event`. */
  emit(event: RealtimeEvent) {
    this.listener(event)
  }
  drop() {
    this.closedListener()
  }
  /** What the window told the voice as context. */
  told(): string[] {
    return this.sent.flatMap((event) => {
      const item = event.item as
        { type?: string; content?: { text: string }[] } | undefined
      return event.type === "conversation.item.create" && item?.type === "message"
        ? [item.content![0].text]
        : []
    })
  }
  outputs(): string[] {
    return this.sent.flatMap((event) => {
      const item = event.item as { type?: string; output?: string } | undefined
      return item?.type === "function_call_output" ? [item.output!] : []
    })
  }
  responses(): number {
    return this.sent.filter((event) => event.type === "response.create").length
  }
}

class FakeConversation implements VoiceConversation {
  readonly sent: string[] = []
  /** The turn each message sent started, in order. */
  readonly turnsSent: string[] = []
  readonly ends = new Map<string, AgentReply>()
  sendAnswer: SendResult = "sent"
  stopAnswer = true
  stops = 0
  turns = "Person: hi\nAgent: hello"
  listeners = new Set<() => void>()
  async send(text: string) {
    this.sent.push(text)
    const turn = `turn-${this.sent.length}`
    this.turnsSent.push(turn)
    return { result: this.sendAnswer, turn }
  }
  async stop() {
    this.stops++
    return this.stopAnswer
  }
  recentTurns() {
    return this.turns
  }
  /** Turns the conversation no longer shows. */
  readonly gone = new Set<string>()
  turnState(turn: string): TurnState {
    const reply = this.ends.get(turn)
    if (this.gone.has(turn)) return { kind: "absent" }
    return reply ? { kind: "ended", reply } : { kind: "open" }
  }
  onChange(listener: () => void) {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }
  /** `turns` (every one sent and not ended, by default) end, saying `reply`. */
  end(reply: AgentReply, turns: readonly string[] = this.turnsSent) {
    for (const turn of turns) if (!this.ends.has(turn)) this.ends.set(turn, reply)
    this.changed()
  }
  changed() {
    for (const listener of this.listeners) listener()
  }
}

function manualTimers() {
  const pending = new Map<number, () => void>()
  let next = 0
  return {
    timers: {
      setTimeout: (run: () => void) => {
        pending.set(++next, run)
        return next
      },
      clearTimeout: (handle: unknown) => void pending.delete(handle as number),
    },
    fire() {
      for (const [handle, run] of [...pending]) {
        pending.delete(handle)
        run()
      }
    },
    pending,
  }
}

function setup(
  options: {
    start?: () => Promise<VoiceCall>
    connect?: (call: VoiceCall, signal: AbortSignal) => Promise<RealtimeConnection>
  } = {},
) {
  const connection = new FakeConnection()
  const conversation = new FakeConversation()
  const clock = manualTimers()
  const startCall = vi.fn(options.start ?? (async () => grant))
  const connect = vi.fn(
    options.connect ?? (async (_call: VoiceCall, _signal: AbortSignal) => connection),
  )
  const controller = new VoiceController({
    gateway: { startCall },
    transports: { webrtc: { connect }, scripted: { connect } },
    conversations: () => conversation,
    timers: clock.timers,
  })
  return { controller, connection, conversation, clock, startCall, connect }
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

/** The person says `transcript` as item `id`, and the voice decides to send it. */
function says(connection: FakeConnection, id: string, transcript: string) {
  connection.emit({ type: "input_audio_buffer.committed", item_id: id })
  connection.emit({
    type: "conversation.item.input_audio_transcription.completed",
    item_id: id,
    transcript,
  })
}

/** The voice's response completes, having called tool `name`. */
function calls(
  connection: FakeConnection,
  name: string,
  callId: string,
  status = "completed",
) {
  connection.emit({
    type: "response.done",
    response: { status, output: [{ type: "function_call", name, call_id: callId }] },
  })
}

/** The voice sends what it heard, and the agent starts on it. */
async function sends(connection: FakeConnection, text: string) {
  says(connection, `heard-${text}`, text)
  calls(connection, "send_to_agent", `call-${text}`)
  await settle()
  connection.emit({ type: "response.done", response: { status: "completed" } })
}

describe("VoiceController", () => {
  it("V1, V4: starts a call for the conversation and gives the voice its context", async () => {
    const { controller, connection, startCall } = setup()
    await controller.start("s-1")
    expect(startCall).toHaveBeenCalledWith("s-1")
    expect(controller.snapshot()).toMatchObject({ phase: "listening", sessionId: "s-1" })
    expect(connection.told()).toEqual(["Chat so far:\nPerson: hi\nAgent: hello"])
    expect(connection.microphone).toBe(true)
    expect(controller.levels()).toEqual({ input: 0.5, output: 0.25 })
  })

  it("V2: a refused start says why and leaves nothing to end", async () => {
    const { controller, connect } = setup({
      start: async () => {
        throw new VoiceStartRefused("Add your OpenAI key in Settings first.")
      },
    })
    await controller.start("s-1")
    expect(controller.snapshot()).toMatchObject({
      phase: "failed",
      notice: "Add your OpenAI key in Settings first.",
    })
    expect(connect).not.toHaveBeenCalled()
    controller.dismiss()
    expect(controller.snapshot().phase).toBe("idle")
  })

  it("V3: a transport that cannot open says why", async () => {
    const { controller } = setup({
      connect: async () => {
        throw new TransportRefused("Nessa can’t use the microphone.")
      },
    })
    await controller.start("s-1")
    expect(controller.snapshot()).toMatchObject({
      phase: "failed",
      notice: "Nessa can’t use the microphone.",
    })
  })

  it("V5: ended while connecting, a late connection is closed", async () => {
    let open!: (connection: RealtimeConnection) => void
    const late = new FakeConnection()
    const { controller } = setup({
      connect: () => new Promise((resolve) => (open = resolve)),
    })
    const starting = controller.start("s-1")
    await settle()
    controller.end()
    open(late)
    await starting
    expect(late.closed).toBe(true)
    expect(controller.snapshot().phase).toBe("idle")
  })

  it("V6, V7, V8: live words show, then the tidied transcript is sent as the person's message", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    connection.emit({ type: "input_audio_buffer.speech_started" })
    connection.emit({
      type: "conversation.item.input_audio_transcription.delta",
      item_id: "i1",
      delta: "um check",
    })
    expect(controller.snapshot().hearing).toBe("um check")
    says(connection, "i1", "um, check the the tests")
    expect(controller.snapshot().hearing).toBe("")
    calls(connection, "send_to_agent", "c1")
    await settle()
    expect(conversation.sent).toEqual(["Check the tests"])
    expect(connection.outputs()).toEqual(["sent"])
    expect(controller.snapshot().phase).toBe("speaking")
    connection.emit({ type: "response.done" })
    expect(controller.snapshot().phase).toBe("working")
  })

  it("V8: everything said since the last send goes as one message, and nothing twice", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    says(connection, "i1", "Open the settings.")
    says(connection, "i2", "And the voice row.")
    calls(connection, "send_to_agent", "c1")
    await settle()
    calls(connection, "send_to_agent", "c2")
    await settle()
    await settle()
    expect(conversation.sent).toEqual(["Open the settings. And the voice row."])
    expect(connection.outputs()).toEqual(["sent", "nothing_heard"])
  })

  it("V9: waits for a transcript the send covers, then sends", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    connection.emit({ type: "input_audio_buffer.committed", item_id: "i1" })
    calls(connection, "send_to_agent", "c1")
    await settle()
    expect(conversation.sent).toEqual([])
    connection.emit({
      type: "conversation.item.input_audio_transcription.completed",
      item_id: "i1",
      transcript: "run the build",
    })
    await settle()
    expect(conversation.sent).toEqual(["Run the build"])
  })

  it("V8: a later request never reaches the agent ahead of one waiting for its words", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    connection.emit({ type: "input_audio_buffer.committed", item_id: "i1" })
    calls(connection, "send_to_agent", "c1")
    await settle()
    says(connection, "i2", "then deploy it")
    calls(connection, "send_to_agent", "c2")
    await settle()
    expect(conversation.sent).toEqual([])
    connection.emit({
      type: "conversation.item.input_audio_transcription.completed",
      item_id: "i1",
      transcript: "first run the tests",
    })
    await settle()
    await settle()
    expect(conversation.sent).toEqual(["First run the tests", "Then deploy it"])
  })

  it("V9: a transcript that never comes is waited for only so long", async () => {
    const { controller, connection, conversation, clock } = setup()
    await controller.start("s-1")
    connection.emit({ type: "input_audio_buffer.committed", item_id: "i1" })
    calls(connection, "send_to_agent", "c1")
    await settle()
    expect(clock.pending.size).toBe(1)
    expect(TRANSCRIPT_WAIT_MS).toBe(5_000)
    clock.fire()
    await settle()
    expect(conversation.sent).toEqual([])
    expect(connection.outputs()).toEqual(["nothing_heard"])
  })

  it("V10: a refused send is answered as not sent", async () => {
    const { controller, connection, conversation } = setup()
    conversation.sendAnswer = "not_sent"
    await controller.start("s-1")
    says(connection, "i1", "deploy it")
    calls(connection, "send_to_agent", "c1")
    await settle()
    expect(connection.outputs()).toEqual(["not_sent"])
    connection.emit({ type: "response.done" })
    expect(controller.snapshot().phase).toBe("listening")
  })

  it("V11, V14: the agent's reply is handed to the voice once the response speaking is done", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    await sends(connection, "run the tests")
    connection.emit({ type: "response.created" })
    conversation.end({ text: "All 12 tests pass.", ended: "finished" })
    expect(connection.told().at(-1)).toBe("Agent reply:\nAll 12 tests pass.")
    expect(connection.responses()).toBe(1)
    connection.emit({ type: "response.done" })
    expect(connection.responses()).toBe(2)
    expect(controller.snapshot().phase).toBe("speaking")
  })

  it("V11: a stopped or failed turn is said as such", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    await sends(connection, "one")
    conversation.end({ text: "", ended: "stopped" })
    connection.emit({ type: "response.done", response: { status: "completed" } })
    await sends(connection, "two")
    conversation.end({ text: "partial", ended: "failed" })
    expect(connection.told().slice(-2)).toEqual([
      "Agent reply: the agent's turn was stopped.",
      "Agent reply: the agent's turn failed.\npartial",
    ])
  })

  it("V13: stop_agent stops the turn as the Stop button does", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    calls(connection, "stop_agent", "c1")
    await settle()
    conversation.stopAnswer = false
    calls(connection, "stop_agent", "c2")
    await settle()
    calls(connection, "rm_rf", "c3")
    await settle()
    expect(conversation.stops).toBe(2)
    expect(connection.outputs()).toEqual(["stopped", "nothing_running", "unknown_tool"])
  })

  it("V15: muting the microphone clears what was half heard", async () => {
    const { controller, connection } = setup()
    await controller.start("s-1")
    controller.setMicrophone(false)
    controller.setSpeaker(false)
    expect(connection.microphone).toBe(false)
    expect(connection.speaker).toBe(false)
    expect(connection.sent.at(-1)).toEqual({ type: "input_audio_buffer.clear" })
    expect(controller.snapshot()).toMatchObject({ microphoneOn: false, speakerOn: false })
  })

  it("V16: ending closes the call and stops listening to the conversation", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    controller.end()
    expect(connection.closed).toBe(true)
    expect(conversation.listeners.size).toBe(0)
    expect(controller.snapshot().phase).toBe("idle")
  })

  it("V16: one call per window: starting another ends the first", async () => {
    const { controller, connection } = setup()
    await controller.start("s-1")
    await controller.start("s-2")
    expect(connection.closed).toBe(true)
    expect(controller.snapshot().sessionId).toBe("s-2")
  })

  it("V17: a call that drops says so", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    connection.drop()
    expect(controller.snapshot()).toMatchObject({
      phase: "dropped",
      notice: "The call dropped.",
    })
    expect(conversation.listeners.size).toBe(0)
  })

  it("an end while a send waits sends nothing", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    connection.emit({ type: "input_audio_buffer.committed", item_id: "i1" })
    calls(connection, "send_to_agent", "c1")
    await settle()
    controller.end()
    await settle()
    expect(conversation.sent).toEqual([])
  })

  it("V11: a turn the voice did not send is not read out", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    conversation.end({ text: "Typed in the composer.", ended: "finished" })
    expect(connection.told().some((text) => text.startsWith("Agent reply:"))).toBe(false)
    expect(controller.snapshot().phase).toBe("listening")
  })

  it("V12: what every turn the voice sent said is handed over once all have ended", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    await sends(connection, "first")
    await sends(connection, "second")
    expect(controller.snapshot().phase).toBe("working")
    conversation.end({ text: "One.", ended: "finished" }, ["turn-1"])
    expect(controller.snapshot().phase).toBe("working")
    expect(connection.told().some((text) => text.startsWith("Agent reply:"))).toBe(false)
    conversation.end({ text: "Two.", ended: "stopped" }, ["turn-2"])
    expect(connection.told().at(-1)).toBe(
      "Agent reply: the agent's turn was stopped.\nOne.\n\nTwo.",
    )
    connection.emit({ type: "response.done", response: { status: "completed" } })
    expect(controller.snapshot().phase).toBe("listening")
    // Handed over once: a later change tells the voice nothing more.
    const told = connection.told().length
    conversation.changed()
    expect(connection.told()).toHaveLength(told)
  })

  it("an end seen once is kept, and a turn shown then gone has ended", async () => {
    const { connection, conversation, controller } = setup()
    await controller.start("s-1")
    await sends(connection, "first")
    await sends(connection, "second")
    await sends(connection, "third")
    conversation.end({ text: "One.", ended: "finished" }, ["turn-1"])
    // The view moves on: the first turn's end and the second turn leave it.
    conversation.gone.add("turn-1")
    conversation.gone.add("turn-2")
    conversation.changed()
    expect(controller.snapshot().phase).toBe("working")
    conversation.end({ text: "Three.", ended: "finished" }, ["turn-3"])
    expect(connection.told().at(-1)).toBe("Agent reply:\nOne.\n\nThree.")
  })

  it("a turn already ended when its send is answered is handed over at once", async () => {
    const { controller, connection, conversation } = setup()
    conversation.ends.set("turn-1", { text: "Quick.", ended: "finished" })
    await controller.start("s-1")
    await sends(connection, "fast")
    expect(connection.told().at(-1)).toBe("Agent reply:\nQuick.")
  })

  it("an unanswered send is said as unknown and not waited on", async () => {
    const { controller, connection, conversation } = setup()
    conversation.sendAnswer = "unknown"
    await controller.start("s-1")
    says(connection, "i1", "deploy it")
    calls(connection, "send_to_agent", "c1")
    await settle()
    expect(connection.outputs()).toEqual(["unknown"])
    connection.emit({ type: "response.done", response: { status: "completed" } })
    expect(controller.snapshot().phase).toBe("listening")
  })

  it("a turn that ends before its send is answered still ends the work", async () => {
    const { controller, connection, conversation } = setup()
    let answer!: (sent: SendResult) => void
    conversation.send = async (text: string) => {
      conversation.sent.push(text)
      conversation.turnsSent.push("turn-1")
      return {
        result: await new Promise<SendResult>((resolve) => (answer = resolve)),
        turn: "turn-1",
      }
    }
    await controller.start("s-1")
    says(connection, "i1", "quick one")
    calls(connection, "send_to_agent", "c1")
    await settle()
    conversation.end({ text: "Done.", ended: "finished" })
    answer("sent")
    await settle()
    // The reply's response, then the one answering the tool.
    connection.emit({ type: "response.done", response: { status: "completed" } })
    connection.emit({ type: "response.done", response: { status: "completed" } })
    expect(controller.snapshot().phase).toBe("listening")
  })

  it("V12: a send queued behind an unanswered one holds back the first turn's reply", async () => {
    const { controller, connection, conversation } = setup()
    const answers: ((sent: SendResult) => void)[] = []
    conversation.send = async (text: string) => {
      conversation.sent.push(text)
      const turn = `turn-${conversation.sent.length}`
      conversation.turnsSent.push(turn)
      return {
        result: await new Promise<SendResult>((resolve) => answers.push(resolve)),
        turn,
      }
    }
    await controller.start("s-1")
    says(connection, "i1", "first")
    calls(connection, "send_to_agent", "c1")
    await settle()
    says(connection, "i2", "second")
    calls(connection, "send_to_agent", "c2")
    await settle()
    // The first turn ends before its send is answered; the second waits.
    conversation.end({ text: "One.", ended: "finished" }, ["turn-1"])
    answers[0]?.("sent")
    await settle()
    expect(connection.told().some((text) => text.startsWith("Agent reply:"))).toBe(false)
    expect(conversation.sent).toEqual(["First", "Second"])
    answers[1]?.("sent")
    await settle()
    conversation.end({ text: "Two.", ended: "finished" }, ["turn-2"])
    expect(connection.told().filter((text) => text.startsWith("Agent reply:"))).toEqual([
      "Agent reply:\nOne.\n\nTwo.",
    ])
  })

  it("V13: a stop withdraws a send still queued and lands after the one on its way", async () => {
    const { controller, connection, conversation } = setup()
    const order: string[] = []
    const answers: ((sent: SendResult) => void)[] = []
    conversation.send = async (text: string) => {
      order.push(`send:${text}`)
      conversation.sent.push(text)
      const turn = `turn-${conversation.sent.length}`
      conversation.turnsSent.push(turn)
      return {
        result: await new Promise<SendResult>((resolve) => answers.push(resolve)),
        turn,
      }
    }
    conversation.stop = async () => {
      order.push("stop")
      return true
    }
    await controller.start("s-1")
    says(connection, "i1", "first")
    calls(connection, "send_to_agent", "c1")
    await settle()
    says(connection, "i2", "second")
    calls(connection, "send_to_agent", "c2")
    await settle()
    calls(connection, "stop_agent", "c3")
    await settle()
    expect(order).toEqual(["send:First"])
    answers[0]?.("sent")
    await settle()
    await settle()
    expect(order).toEqual(["send:First", "stop"])
    expect(connection.outputs()).toContain("not_sent")
    expect(connection.outputs()).toContain("stopped")
  })

  it("a tool asked for by a response the person talked over is not called", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    calls(connection, "stop_agent", "c1", "cancelled")
    await settle()
    expect(conversation.stops).toBe(0)
    expect(connection.outputs()).toEqual([])
  })

  it("V14: a response the provider refused frees the voice to speak", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    await sends(connection, "go")
    // The reply asks for a response; the provider refuses it.
    conversation.end({ text: "Done.", ended: "finished" })
    expect(controller.snapshot().phase).toBe("speaking")
    // Another request's refusal changes nothing.
    connection.emit({ type: "error", error: { message: "bad item", event_id: "other" } })
    expect(controller.snapshot().phase).toBe("speaking")
    const asked = connection.sent
      .filter((event) => event.type === "response.create")
      .at(-1)
    connection.emit({
      type: "error",
      error: { message: "rate limited", event_id: asked?.event_id },
    })
    expect(controller.snapshot().phase).toBe("listening")
    await sends(connection, "again")
    expect(connection.responses()).toBe(3)
  })

  it("V9: a transcript that failed is nothing heard, at once", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    connection.emit({ type: "input_audio_buffer.committed", item_id: "i1" })
    calls(connection, "send_to_agent", "c1")
    await settle()
    connection.emit({
      type: "conversation.item.input_audio_transcription.failed",
      item_id: "i1",
    })
    await settle()
    await settle()
    expect(conversation.sent).toEqual([])
    expect(connection.outputs()).toEqual(["nothing_heard"])
  })

  it("V16: the call ends once no pane shows its conversation", async () => {
    const { controller, connection } = setup()
    const first = controller.show("s-1")
    const second = controller.show("s-1")
    await controller.start("s-1")
    first()
    first()
    expect(connection.closed).toBe(false)
    second()
    expect(connection.closed).toBe(true)
    expect(controller.snapshot().phase).toBe("idle")
  })

  it("V5: ending while connecting aborts the connect", async () => {
    let signal!: AbortSignal
    const { controller } = setup({
      connect: (_call, given) => {
        signal = given
        return new Promise(() => {})
      },
    })
    void controller.start("s-1")
    await settle()
    expect(signal.aborted).toBe(false)
    controller.end()
    expect(signal.aborted).toBe(true)
  })

  it("V14: a response refused because the voice is already answering is asked again after", async () => {
    const { controller, connection, conversation } = setup()
    await controller.start("s-1")
    await sends(connection, "go")
    conversation.end({ text: "Done.", ended: "finished" })
    const asked = connection.sent
      .filter((event) => event.type === "response.create")
      .at(-1)
    // The provider answers the person by itself first, then refuses ours.
    connection.emit({ type: "response.created" })
    connection.emit({
      type: "error",
      error: {
        code: "conversation_already_has_active_response",
        event_id: asked?.event_id,
      },
    })
    const before = connection.responses()
    connection.emit({ type: "response.done", response: { status: "completed" } })
    expect(connection.responses()).toBe(before + 1)
    expect(controller.snapshot().phase).toBe("speaking")
  })
})
