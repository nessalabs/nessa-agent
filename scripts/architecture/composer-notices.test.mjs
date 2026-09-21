import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

import {
  composerBudgetViolations,
  composerChildren,
  composerNoticeViolations,
  declaredNoticeCap,
  maskJsxNonCode,
} from "./composer-notices.mjs"

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..")
const read = (path) => readFileSync(join(root, path), "utf8")

const composer = (children) => `
  <div className="nessa-composer" hidden={update.viewing || undefined}>
    ${children}
  </div>
`

test("a notice handed to the box is inside it, not beside it", () => {
  const source = composer(`
    <ComposerNotices
      link={link.notice ? <AgentNotification title={link.notice.title} /> : null}
      attachments={<AttachmentNotices refusal={attachments.refusal} />}
      conversation={<ConversationNotification conversation={chat.active} />}
    />
    <ConversationQueue conversation={chat.active} />
    {generating && <ComposerDeliveryMode value={chat.deliveryMode} />}
    <PillComposer key={chat.active.id}>
      <ChatComposerAttachments>
        {files.map((file) => (
          <AttachmentTile key={file.id} file={file} />
        ))}
      </ChatComposerAttachments>
    </PillComposer>
  `)
  assert.deepEqual(composerChildren(source), [
    "ComposerNotices",
    "ConversationQueue",
    "ComposerDeliveryMode",
    "PillComposer",
  ])
  assert.deepEqual(composerNoticeViolations("src/panel/ui/app.tsx", source), [])
})

test("a sixth notice pasted in beside the box is refused", () => {
  const source = composer(`
    <ComposerNotices link={null} />
    <AgentNotification title="Something else" />
    <ConversationQueue />
    {generating && <ComposerDeliveryMode />}
    <PillComposer />
  `)
  const [failure] = composerNoticeViolations("src/panel/ui/app.tsx", source)
  assert.match(failure, /ComposerNotices/)
  assert.match(failure, /AgentNotification/)
})

test("prose and strings are not tags", () => {
  // The chrome is heavily commented and the comments talk about markup.
  const masked = maskJsxNonCode(`{/* <AgentNotification> goes here */}\n<Real />`)
  assert.equal(masked.includes("AgentNotification"), false)
  assert.equal(masked.includes("<Real />"), true)
  const source = composer(`
    {/* A notice would go in <ComposerNotices>, never here. */}
    <ComposerNotices link={null} />
    <ConversationQueue />
    {generating && <ComposerDeliveryMode />}
    <PillComposer placeholder="a < b" />
  `)
  assert.deepEqual(composerNoticeViolations("src/panel/ui/app.tsx", source), [])
})

test("a renamed composer element fails loudly rather than passing empty", () => {
  const [failure] = composerNoticeViolations(
    "src/panel/ui/app.tsx",
    `<div className="nessa-composer-pane"><PillComposer /></div>`,
  )
  assert.match(failure, /no longer carries/)
  // Only the chrome is held to this.
  assert.deepEqual(composerNoticeViolations("src/panel/ui/other.tsx", "anything"), [])
})

test("the ceiling holds at every size the window can be", () => {
  const cap = declaredNoticeCap(read("src/styles.css"))
  assert.notEqual(cap, null, "styles.css no longer caps the notice strip")
  // The real bounds: the window's own configured minimum, the shipped size,
  // and the default height. At each of them the notices get at most a third,
  // which is what leaves the transcript and the pill the rest.
  const window = JSON.parse(read("src-tauri/tauri.conf.json")).app.windows[0]
  assert.equal(window.minHeight, 320)
  assert.equal(window.minWidth, 420)
  for (const height of [window.minHeight, 420, window.height]) {
    assert.ok(
      cap(height) <= height / 3,
      `at ${height}px the notices may take ${cap(height)}px`,
    )
  }
  // 106px at the shortest the window goes: one card and the top of the next,
  // rather than four cards and no composer.
  assert.equal(Math.round(cap(window.minHeight)), 107)
})

test("the shipped stylesheet declares the whole contract", () => {
  assert.deepEqual(composerBudgetViolations(read("src/styles.css")), [])
})

test("a ceiling without a scrollbar, or with no ceiling at all, is refused", () => {
  const clipped = `.nessa-composer-notices { max-height: calc(var(--nessa-window-height, 100vh) / 3); }
    .nessa-composer-notices:empty { display: none; }`
  assert.match(composerBudgetViolations(clipped)[0], /must scroll/)

  const halfThePanel = `.nessa-composer-notices { max-height: calc(var(--nessa-window-height, 100vh) / 2); overflow-y: auto; }
    .nessa-composer-notices:empty { display: none; }`
  assert.match(composerBudgetViolations(halfThePanel)[0], /at most a third/)

  const fixed = `.nessa-composer-notices { max-height: 200px; overflow-y: auto; }
    .nessa-composer-notices:empty { display: none; }`
  assert.match(composerBudgetViolations(fixed)[0], /--nessa-window-height/)

  assert.match(composerBudgetViolations(".nessa-composer { padding: 0 }")[0], /no rule/)
})

test("the chrome and the stylesheet that ship pass their own rules", () => {
  assert.deepEqual(
    composerNoticeViolations("src/panel/ui/app.tsx", read("src/panel/ui/app.tsx")),
    [],
  )
})
