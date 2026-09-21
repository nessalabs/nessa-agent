import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

import {
  composerBudgetViolations,
  composerChildren,
  composerNoticeViolations,
  declaredCapFallbacks,
  declaredNoticeCap,
  noticeRules,
} from "./composer-notices.mjs"

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..")
const read = (path) => readFileSync(join(root, path), "utf8")
const chrome = "src/panel/ui/app.tsx"

const composer = (children) => `
export function App() {
  return (
    <div className="nessa-composer" hidden={update.viewing || undefined}>
      ${children}
    </div>
  )
}
`

const shipped = `
    <ComposerNotices
      link={link.notice ? <AgentNotification title={link.notice.title} /> : null}
      attachments={<AttachmentNotices refusal={attachments.refusal} />}
      conversation={<ConversationNotification conversation={chat.active} />}
    />
    <ConversationQueue conversation={chat.active} />
    {generating && <ComposerDeliveryMode value={chat.deliveryMode} />}
    <PillComposer key={chat.active.id} />
`

test("a notice handed to the box is inside it, not beside it", () => {
  const source = composer(shipped)
  assert.deepEqual(composerChildren(chrome, source), [
    "ComposerNotices",
    "ConversationQueue",
    "ComposerDeliveryMode",
    "PillComposer",
  ])
  assert.deepEqual(composerNoticeViolations(chrome, source), [])
})

test("a notice pasted in beside the box is refused", () => {
  const source = composer(`
    <ComposerNotices link={null} />
    <AgentNotification title="Something else" />
    <ConversationQueue />
    {generating && <ComposerDeliveryMode />}
    <PillComposer />
  `)
  const failures = composerNoticeViolations(chrome, source)
  assert.match(failures.join("\n"), /the composer holds/)
  assert.match(failures.join("\n"), /outside ComposerNotices \(line \d+\)/)
})

test("a fragment cannot end the scan early and let a notice through", () => {
  // The defect this parser replaced. To a bracket count `<>` is not a tag and
  // `</>` looks like the end of one, so an ordinary fragment inside the pill
  // drove the depth to zero, the scan stopped there, and everything after it —
  // including a notice pasted in beside the box — was never looked at. The
  // check passed while the rule it exists for was being broken.
  const source = composer(`
    <ComposerNotices link={null} />
    <ConversationQueue />
    {generating && <ComposerDeliveryMode />}
    <PillComposer>
      <>
        <AttachmentTile />
        <AttachmentTile />
      </>
    </PillComposer>
    <AgentNotification title="Slipped past" />
  `)
  const failures = composerNoticeViolations(chrome, source)
  assert.match(failures.join("\n"), /outside ComposerNotices/)
  assert.match(failures.join("\n"), /the composer holds .*AgentNotification/)
})

test("a fragment in a slot, a generic and a comparison are not a violation", () => {
  // The same defect in the other direction: each of these truncated the scan
  // and produced a refusal whose message was a false statement about the file.
  const source = composer(`
    <ComposerNotices
      link={
        <>
          <AgentNotification title="One" />
          <AgentNotification title="Two" />
        </>
      }
    />
    <ConversationQueue count={waiting.length} />
    {generating && files.length < 3 && <ComposerDeliveryMode />}
    <PillComposer onDone={useMemo<Ref<Item>>(() => done, [])} />
  `)
  assert.deepEqual(composerNoticeViolations(chrome, source), [])
})

test("prose about markup is prose", () => {
  const source = composer(`
    {/* A notice would go in <ComposerNotices>, never <AgentNotification> here. */}
    <ComposerNotices link={null} />
    <ConversationQueue />
    {generating && <ComposerDeliveryMode />}
    <PillComposer placeholder="a < b, </div>" />
  `)
  assert.deepEqual(composerNoticeViolations(chrome, source), [])
})

test("a notice handed to something else that renders it is still outside the box", () => {
  // What the child list alone cannot see: the queue is a permitted child, so a
  // notice passed to it as a prop keeps the list correct and the column
  // unbounded.
  const source = composer(`
    <ComposerNotices link={null} />
    <ConversationQueue banner={<AgentNotification title="Hidden here" />} />
    {generating && <ComposerDeliveryMode />}
    <PillComposer />
  `)
  const failures = composerNoticeViolations(chrome, source)
  assert.equal(failures.length, 1)
  assert.match(failures[0], /outside ComposerNotices/)
})

test("a renamed composer element fails loudly rather than passing empty", () => {
  const [failure] = composerNoticeViolations(
    chrome,
    `const App = () => <div className="nessa-composer-pane"><PillComposer /></div>`,
  )
  assert.match(failure, /no longer carries/)
  // Only the chrome is held to this.
  assert.deepEqual(composerNoticeViolations("src/panel/ui/other.tsx", "anything"), [])
})

test("the ceiling holds at every size the window can be", () => {
  const styles = read("src/styles.css")
  const cap = declaredNoticeCap(styles)
  assert.notEqual(cap, null, "styles.css no longer caps the notice strip")
  // The real bounds: the window's own configured minimum, the shipped width,
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
  // 107px at the shortest the window goes: one card and the top of the next,
  // rather than four cards and no composer.
  assert.equal(Math.round(cap(window.minHeight)), 107)
})

test("the guess before the host reports a size is the window, not the screen", () => {
  // The webview is sized to the display's work area and never smaller than the
  // window (`platform/macos/viewport.rs`), so `100vh` in the app is the screen.
  // A third of a screen is taller than a 320px panel, which is no cap at all
  // for the frames before `windowSize()` resolves.
  const window = JSON.parse(read("src-tauri/tauri.conf.json")).app.windows[0]
  const fallbacks = declaredCapFallbacks(read("src/styles.css"))
  const hosted = fallbacks.filter((cap) => /data-host=/.test(cap.selector))
  assert.ok(hosted.length > 0, "the Tauri hosts need a fallback that is not the viewport")
  for (const cap of hosted) {
    assert.equal(cap.fallback, `${window.minHeight}px`)
  }
  for (const host of ["linux", "macos"]) {
    assert.ok(
      hosted.some((cap) => cap.selector.includes(`data-host="${host}"`)),
      `${host} still guesses the viewport`,
    )
  }
  // A browser preview keeps the viewport, where the viewport really is the window.
  assert.ok(fallbacks.some((cap) => cap.fallback === "100vh"))
})

test("the shipped stylesheet declares the whole contract", () => {
  assert.deepEqual(composerBudgetViolations(read("src/styles.css")), [])
})

test("the ways the ceiling is actually lost are all refused", () => {
  const base = `.nessa-composer-notices { max-height: calc(var(--nessa-window-height, 100vh) / 3); overflow-y: auto; }
    .nessa-composer-notices:empty { display: none; }`
  assert.deepEqual(composerBudgetViolations(base), [])

  // Every one of these used to pass, because only the first block was read.
  const overridden = `${base}
    .nessa-panel .nessa-composer-notices { max-height: 100vh; }`
  assert.match(composerBudgetViolations(overridden)[0], /max-height: 100vh/)

  const inMedia = `${base}
    @media (max-height: 500px) { .nessa-composer-notices { max-height: none; } }`
  assert.match(composerBudgetViolations(inMedia)[0], /max-height: none/)

  // A floor beats the ceiling outright, in the very same block.
  const floored = `.nessa-composer-notices { max-height: calc(var(--nessa-window-height, 100vh) / 3); min-height: 400px; overflow-y: auto; }
    .nessa-composer-notices:empty { display: none; }`
  assert.match(composerBudgetViolations(floored)[0], /min-height: 400px/)

  const stopped = `${base}
    .nessa-panel .nessa-composer-notices { overflow-y: visible; }`
  assert.match(composerBudgetViolations(stopped)[0], /vertical overflow to visible/)
})

test("the shorthand the build itself emits is a scrolling box", () => {
  // Lightning CSS collapses the two axes into `overflow: hidden auto`, and a
  // gate that refused its own output would be a gate nobody could trust.
  const shorthand = `.nessa-composer-notices { max-height: calc(var(--nessa-window-height, 100vh) / 3); overflow: hidden auto; }
    .nessa-composer-notices:empty { display: none; }`
  assert.deepEqual(composerBudgetViolations(shorthand), [])
})

test("a comment naming the box is not a rule for it", () => {
  const styles = `/* .nessa-composer-notices has a min-height: none of your business */
    .nessa-composer { padding: 0 16px 12px; }`
  assert.deepEqual(noticeRules(styles), [])
  assert.match(composerBudgetViolations(styles)[0], /no rule/)
})

test("half the panel, a fixed number, and no ceiling at all are refused", () => {
  const half = `.nessa-composer-notices { max-height: calc(var(--nessa-window-height, 100vh) / 2); overflow-y: auto; }
    .nessa-composer-notices:empty { display: none; }`
  assert.match(composerBudgetViolations(half)[0], /50% share/)

  const fixed = `.nessa-composer-notices { max-height: 200px; overflow-y: auto; }
    .nessa-composer-notices:empty { display: none; }`
  assert.match(composerBudgetViolations(fixed)[0], /--nessa-window-height/)

  const uncapped = `.nessa-composer-notices { overflow-y: auto; }
    .nessa-composer-notices:empty { display: none; }`
  assert.match(composerBudgetViolations(uncapped)[0], /need a max-height/)

  const unscrolled = `.nessa-composer-notices { max-height: calc(var(--nessa-window-height, 100vh) / 3); }
    .nessa-composer-notices:empty { display: none; }`
  assert.match(composerBudgetViolations(unscrolled)[0], /must scroll/)

  const alwaysThere = `.nessa-composer-notices { max-height: calc(var(--nessa-window-height, 100vh) / 3); overflow-y: auto; }`
  assert.match(composerBudgetViolations(alwaysThere)[0], /display:none/)
})

test("the chrome and the stylesheet that ship pass their own rules", () => {
  assert.deepEqual(composerNoticeViolations(chrome, read(chrome)), [])
  assert.deepEqual(composerBudgetViolations(read("src/styles.css")), [])
})
