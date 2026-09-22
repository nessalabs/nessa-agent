import { describe, expect, it } from "vitest"

import {
  conversationId,
  conversationMutation,
  conversationReceipt,
  conversationReorder,
  conversationView,
} from "./conversation-validate.js"

const DIGEST = `sha256:${"0".repeat(64)}`
function image(change: { size?: number } = {}) {
  return { digest: DIGEST, mimeType: "image/png", size: 3, ...change }
}

function view() {
  return {
    conversationId: "conversation",
    revision: "1",
    truncated: false,
    queueComplete: true,
    messages: [
      {
        executionId: "queued",
        userText: "hello",
        attachments: [],
        files: [],
        status: "queued",
        parts: [],
      },
      {
        executionId: "running",
        userText: "run",
        attachments: [],
        files: [],
        status: "running",
        parts: [{ offset: 0, kind: "tool", text: "", toolId: "tool" }],
      },
    ],
    pending: [
      {
        executionId: "queued",
        text: "hello",
        attachments: [],
        files: [],
        mode: "queued",
      },
    ],
    permissions: [],
    tools: [
      {
        executionId: "running",
        toolId: "tool",
        title: "Tool",
        status: "running",
        details: "",
        input: "{}",
      },
    ],
    capabilities: {
      queue: true,
      steer: true,
      resume: true,
      permissions: true,
      imageInput: false,
    },
  }
}

describe("conversation view agreement", () => {
  it("accepts complete matching pending and tool evidence", () => {
    expect(conversationView(view(), "conversation").revision).toBe("1")
  })

  it("rejects contradictory pending text when queue evidence is complete", () => {
    const value = view()
    value.pending[0]!.text = "different"
    expect(() => conversationView(value, "conversation")).toThrow(
      "Pending execution contradicts",
    )
  })

  it("accepts matching pending and message text above the former preview limit", () => {
    const value = view()
    const text = "😀".repeat(2048)
    value.messages[0]!.userText = text
    value.pending[0]!.text = text

    expect(new TextEncoder().encode(text)).toHaveLength(8192)
    expect(() => conversationView(value, "conversation")).not.toThrow()
  })

  it("allows pending text mismatch when bounded evidence is truncated", () => {
    const value = view()
    value.truncated = true
    value.pending[0]!.text = "bounded preview"
    expect(() => conversationView(value, "conversation")).not.toThrow()
  })

  it("rejects orphan and cross-execution tool state in complete views", () => {
    const orphan = view()
    orphan.messages[1]!.parts = []
    expect(() => conversationView(orphan, "conversation")).toThrow("orphaned")

    const mismatch = view()
    mismatch.tools[0]!.executionId = "queued"
    expect(() => conversationView(mismatch, "conversation")).toThrow()
  })

  it("allows bounded tool omissions only when the view says it is truncated", () => {
    const value = view()
    value.truncated = true
    value.tools = []
    expect(() => conversationView(value, "conversation")).not.toThrow()
  })

  it("rejects unknown fields at the view and nested schema boundaries", () => {
    const mutations: Array<(value: ReturnType<typeof view>) => void> = [
      (value) => Object.assign(value, { extra: true }),
      (value) => Object.assign(value.messages[0]!, { extra: true }),
      (value) => Object.assign(value.messages[1]!.parts[0]!, { extra: true }),
      (value) => Object.assign(value.pending[0]!, { extra: true }),
      (value) => Object.assign(value.tools[0]!, { extra: true }),
      (value) => Object.assign(value.capabilities, { extra: true }),
    ]
    for (const mutate of mutations) {
      const value = view()
      mutate(value)
      expect(() => conversationView(value, "conversation")).toThrow("unknown fields")
    }

    const permission = view()
    Object.assign(permission, {
      permissions: [
        {
          executionId: "running",
          permissionId: "permission",
          toolId: "tool",
          title: "Review",
          toolName: "write_file",
          argumentsJson: "{}",
          options: [{ id: "allow", label: "Allow", extra: true }],
        },
      ],
    })
    expect(() => conversationView(permission, "conversation")).toThrow("unknown fields")
  })

  it("accepts an image-only message whose waiting input names the same images", () => {
    const value = view()
    value.messages[0]!.userText = ""
    value.pending[0]!.text = ""
    Object.assign(value.messages[0]!, { attachments: [image()] })
    // Field order belongs to the serializer; the same image is the same image.
    Object.assign(value.pending[0]!, {
      attachments: [{ size: 3, mimeType: "image/png", digest: DIGEST }],
      files: [],
    })
    const checked = conversationView(value, "conversation")
    expect(checked.messages[0]!.attachments).toEqual([image()])
  })

  it("rejects waiting images that contradict their queued message", () => {
    const value = view()
    Object.assign(value.messages[0]!, { attachments: [image()] })
    Object.assign(value.pending[0]!, { attachments: [image({ size: 4 })] })
    expect(() => conversationView(value, "conversation")).toThrow(
      "Pending execution contradicts",
    )
    const bounded = view()
    bounded.truncated = true
    Object.assign(bounded.pending[0]!, { attachments: [image()] })
    expect(() => conversationView(bounded, "conversation")).not.toThrow()
  })

  it.each([
    ["an uppercase digest", { digest: `sha256:${"A".repeat(64)}` }],
    ["a bare hex digest", { digest: "0".repeat(64) }],
    ["a short digest", { digest: "sha256:00" }],
    ["a media type no message may carry", { mimeType: "image/svg+xml" }],
    ["a non-image media type", { mimeType: "application/pdf" }],
    ["an empty image", { size: 0 }],
    ["a fractional size", { size: 1.5 }],
    ["an image one byte over the schema's maximum", { size: 5_242_881 }],
    ["an unknown field", { bytes: "AAAA" }],
  ])("rejects a message image with %s", (_name, change) => {
    for (const where of ["messages", "pending"] as const) {
      const value = view()
      value.truncated = true
      Object.assign(value[where][0]!, { attachments: [{ ...image(), ...change }] })
      expect(() => conversationView(value, "conversation")).toThrow("attachments")
    }
  })

  it("rejects more images, or more image bytes, than one message may carry", () => {
    const eleven = view()
    eleven.truncated = true
    Object.assign(eleven.messages[0]!, {
      attachments: Array.from({ length: 11 }, () => image()),
    })
    expect(() => conversationView(eleven, "conversation")).toThrow("at most 10")

    const heavy = view()
    heavy.truncated = true
    Object.assign(heavy.messages[0]!, {
      attachments: Array.from({ length: 3 }, () => image({ size: 4 * 1024 * 1024 })),
    })
    expect(() => conversationView(heavy, "conversation")).toThrow("bytes of images")

    const exact = view()
    exact.truncated = true
    Object.assign(exact.messages[0]!, {
      attachments: Array.from({ length: 2 }, () => image({ size: 5 * 1024 * 1024 })),
    })
    expect(() => conversationView(exact, "conversation")).not.toThrow()
  })

  it("requires attachments and the image capability rather than assuming them", () => {
    const message = view()
    delete (message.messages[0] as { attachments?: unknown }).attachments
    expect(() => conversationView(message, "conversation")).toThrow("attachments")

    const pending = view()
    delete (pending.pending[0] as { attachments?: unknown }).attachments
    expect(() => conversationView(pending, "conversation")).toThrow("attachments")

    const capability = view()
    delete (capability.capabilities as { imageInput?: boolean }).imageInput
    expect(() => conversationView(capability, "conversation")).toThrow("imageInput")

    const truthy = view()
    Object.assign(truthy.capabilities, { imageInput: "true" })
    expect(() => conversationView(truthy, "conversation")).toThrow("imageInput")
  })

  it("rejects unknown fields in receipts and control results", () => {
    expect(() => conversationId({ conversationId: "c", extra: true }, "c")).toThrow()
    expect(() =>
      conversationReceipt({ executionId: "e", disposition: "queued", extra: true }, "e"),
    ).toThrow()
    expect(() =>
      conversationMutation({ requestId: "r", applied: true, extra: true }, "r"),
    ).toThrow()
    expect(() =>
      conversationReorder({ requestId: "r", outcome: "applied", extra: true }, "r"),
    ).toThrow()
  })
})
