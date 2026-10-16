// @vitest-environment jsdom
import { act } from "react"
import { createRoot } from "react-dom/client"
import { beforeEach, expect, it } from "vitest"
import { transcriptFrom } from "../../adapters/gateway/gateway-views"
import { view } from "../../adapters/gateway/fake-gateway"
import { Message } from "./message"

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
})

/**
 * A reply as the gateway streams it: one provider message in fragments that
 * break mid-line and mid-link, so the structure exists only once they are
 * joined, exactly as written.
 */
const fragments = [
  "## Where to go\n\nThree places worth a day:\n\n",
  "- [Old Montreal](https://example.com/old) — cobbled streets\n",
  "- **Mount Royal Park** — the view, `free`\n\n",
  "1. Morning\n2. Afternoon\n\nSee the [official ",
  "site](https://example.com/site).\nA line after a break.\n\n",
  "```sh\necho hi\n```\n",
]

async function drawn(parts: readonly string[]) {
  const transcript = transcriptFrom(
    view("c", {
      messages: [
        {
          executionId: "e1",
          userText: "Plan a day",
          attachments: [],
          files: [],
          status: "completed",
          parts: parts.map((text, offset) => ({
            offset,
            kind: "text" as const,
            text,
            toolId: "",
            noticeId: "",
            messageId: "m1",
          })),
        },
      ],
    }),
    1,
    () => 1,
    1000,
  )
  const agent = transcript.messages.find((message) => message.role === "agent")
  if (!agent) throw new Error("no agent message")
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  await act(async () =>
    root.render(<Message sessionId="c" message={agent} isNew={false} />),
  )
  const body = host.querySelector(".workspace-message-body")
  if (!body) throw new Error("no message body")
  return {
    body,
    async dispose() {
      await act(async () => root.unmount())
      host.remove()
    },
  }
}

it("draws an agent's Markdown reply as blocks, not one run-on paragraph", async () => {
  const { body, dispose } = await drawn(fragments)
  try {
    expect(body.querySelector("h2")?.textContent).toBe("Where to go")
    const bullets = [...body.querySelectorAll("ul > li")]
    expect(bullets.map((item) => item.textContent)).toEqual([
      "Old Montreal — cobbled streets",
      "Mount Royal Park — the view, free",
    ])
    expect(bullets[1]?.querySelector("strong")?.textContent).toBe("Mount Royal Park")
    expect(bullets[1]?.querySelector("code")?.textContent).toBe("free")
    expect([...body.querySelectorAll("ol > li")].map((item) => item.textContent)).toEqual(
      ["Morning", "Afternoon"],
    )
    const links = [...body.querySelectorAll("a")].map((link) => [
      link.textContent,
      link.getAttribute("href"),
      link.getAttribute("target"),
    ])
    expect(links).toEqual([
      ["Old Montreal", "https://example.com/old", null],
      ["official site", "https://example.com/site", null],
    ])
    // A single newline inside a paragraph is a soft break; blank lines part paragraphs.
    const paragraphs = [...body.querySelectorAll(":scope > div > p")].map(
      (p) => p.textContent,
    )
    expect(paragraphs).toEqual([
      "Three places worth a day:",
      "See the official site.\nA line after a break.",
    ])
    expect(body.textContent).not.toContain("](")
    expect(body.textContent).not.toContain("## ")
    // Fenced code goes to the design system's code surface, which highlights lazily.
    expect(body.querySelector("[data-slot=code-block]")).not.toBeNull()
  } finally {
    await dispose()
  }
})

it("renders no HTML the agent wrote, and no script link", async () => {
  const { body, dispose } = await drawn([
    '<a href="https://example.com" target="_blank">raw</a> and [x](javascript:alert(1))',
  ])
  try {
    expect(body.querySelector("a[target]")).toBeNull()
    expect(
      [...body.querySelectorAll("a")].some((link) =>
        (link.getAttribute("href") ?? "").startsWith("javascript:"),
      ),
    ).toBe(false)
  } finally {
    await dispose()
  }
})

it("draws nothing for a reply that is only whitespace so far", async () => {
  const { body, dispose } = await drawn(["\n\n"])
  try {
    expect(body.querySelector("[data-slot=message-markdown]")).toBeNull()
  } finally {
    await dispose()
  }
})

function streamed() {
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  const prose = () => host.querySelector(".workspace-prose")?.textContent ?? ""
  const draw = (text: string, working: boolean, isNew: boolean) =>
    act(async () =>
      root.render(
        <Message
          sessionId="c"
          isNew={isNew}
          message={{
            id: "r",
            role: "agent",
            at: 1,
            parts: [{ kind: "text", text }],
            ...(working ? { working: true as const } : {}),
          }}
        />,
      ),
    )
  const frame = () => act(async () => new Promise((done) => setTimeout(done, 20)))
  const done = async () => {
    await act(async () => root.unmount())
    host.remove()
  }
  return { host, prose, draw, frame, done }
}

it("reveals a new reply's words over frames, and its last words never go back", async () => {
  const view = streamed()
  await view.draw("Three places ", true, true)
  // Its first chunk, like any burst, is released over frames, not set down in one.
  expect(view.prose()).not.toBe("Three places ")
  // Revealed in full before more arrives: the reveal has caught up.
  for (let wait = 0; wait < 60 && view.prose().trim() !== "Three places"; wait++)
    await view.frame()
  expect(view.prose().trim()).toBe("Three places")
  // The last words land with the turn's end, in one render.
  await view.draw("Three places worth a day.", false, true)
  const seen = [view.prose().length]
  // Watched past the reveal and its fades, not only until it first reads whole.
  for (let wait = 0; wait < 80; wait++) {
    await view.frame()
    seen.push(view.prose().length)
  }
  expect(seen).toEqual([...seen].sort((a, b) => a - b))
  expect(view.prose()).toBe("Three places worth a day.")
  // Every fade finished, the reply is plain text again.
  expect(view.host.querySelector(".workspace-prose span")).toBeNull()
  await view.done()
})

it("shows at once what a reply already said when it is opened mid-stream", async () => {
  const view = streamed()
  await view.draw("Three places worth a day.", true, false)
  expect(view.prose()).toBe("Three places worth a day.")
  await view.done()
})
