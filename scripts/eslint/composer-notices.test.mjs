/**
 * These run under `pnpm lint:rules`, in the frontend job, because a rule tester
 * needs ESLint itself. The architecture script's tests run on the Rust jobs
 * with bare Node and must stay dependency-free; that is the whole reason this
 * rule is here and not there.
 */
import test from "node:test"
import { RuleTester } from "eslint"
import tseslint from "typescript-eslint"

import { composerNotices } from "./composer-notices.mjs"

const tester = new RuleTester({
  languageOptions: {
    parser: tseslint.parser,
    ecmaVersion: "latest",
    sourceType: "module",
    parserOptions: { ecmaFeatures: { jsx: true } },
  },
})

const composer = (children) => `
export function App() {
  return (
    <div className="nessa-composer" hidden={update.viewing || undefined}>
      ${children}
    </div>
  )
}
`

const shipped = composer(`
  <ComposerNotices
    link={link.notice ? <AgentNotification title={link.notice.title} /> : null}
    attachments={<AttachmentNotices refusal={attachments.refusal} />}
    conversation={<ConversationNotification conversation={chat.active} />}
  />
  <ConversationQueue conversation={chat.active} />
  {generating && <ComposerDeliveryMode value={chat.deliveryMode} />}
  <PillComposer key={chat.active.id} />
`)

const run = (cases) => tester.run("composer-notices", composerNotices, cases)

test("the shape that ships is allowed", () => {
  run({ valid: [shipped], invalid: [] })
})

test("valid code that used to disarm a bracket count is still valid", () => {
  // The defect this rule replaced counted `<` and `>`. A fragment broke it in
  // both directions — `<>` is not a tag to that kind of scan while `</>` looks
  // like the end of one — and a generic argument and a `<` comparison each
  // corrupted it as well. ESLint parses all of it.
  run({
    valid: [
      composer(`
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
      `),
      composer(`
        {/* A notice would go in <ComposerNotices>, never <AgentNotification> here. */}
        <ComposerNotices link={null} />
        <ConversationQueue />
        {generating && <ComposerDeliveryMode />}
        <PillComposer placeholder="a < b, </div>" />
      `),
    ],
    invalid: [],
  })
})

test("a notice pasted in beside the box is caught twice", () => {
  run({
    valid: [],
    invalid: [
      {
        code: composer(`
          <ComposerNotices link={null} />
          <AgentNotification title="Something else" />
          <ConversationQueue />
          {generating && <ComposerDeliveryMode />}
          <PillComposer />
        `),
        errors: [{ messageId: "layout" }, { messageId: "outside" }],
      },
    ],
  })
})

test("a fragment in the pill cannot end the scan and let a notice through", () => {
  // The fail-open half of the defect, and the one that mattered: the bracket
  // count reached zero on `</>` and stopped, so everything after it — a notice
  // included — was never looked at, and the check passed.
  run({
    valid: [],
    invalid: [
      {
        code: composer(`
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
        `),
        errors: [{ messageId: "layout" }, { messageId: "outside" }],
      },
    ],
  })
})

test("a notice handed to another child that renders it is still outside the box", () => {
  // What the list of direct children alone cannot see: the queue is a permitted
  // child, so a notice passed to it as a prop keeps the list correct and the
  // column unbounded.
  run({
    valid: [],
    invalid: [
      {
        code: composer(`
          <ComposerNotices link={null} />
          <ConversationQueue banner={<AgentNotification title="Hidden here" />} />
          {generating && <ComposerDeliveryMode />}
          <PillComposer />
        `),
        errors: [{ messageId: "outside" }],
      },
    ],
  })
})

test("reordering the composer's children is a change to the budget", () => {
  run({
    valid: [],
    invalid: [
      {
        code: composer(`
          <ConversationQueue />
          <ComposerNotices link={null} />
          {generating && <ComposerDeliveryMode />}
          <PillComposer />
        `),
        errors: [{ messageId: "layout" }],
      },
      {
        code: composer(`
          <ComposerNotices link={null} />
          <div className="extra-row" />
          <ConversationQueue />
          {generating && <ComposerDeliveryMode />}
          <PillComposer />
        `),
        errors: [{ messageId: "layout" }],
      },
    ],
  })
})

test("a renamed composer element fails loudly rather than passing empty", () => {
  run({
    valid: [],
    invalid: [
      {
        code: `export const App = () => <div className="nessa-composer-pane"><PillComposer /></div>`,
        errors: [{ messageId: "missing" }],
      },
    ],
  })
})
