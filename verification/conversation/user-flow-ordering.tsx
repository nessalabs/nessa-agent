/** Actual downloads component and composer hook over deterministic application ports. */
import * as React from "react"
import { createRoot } from "react-dom/client"
import { AgentDownloads } from "../../src/onboarding/ui/agent-downloads"
import type {
  AgentInstallations,
  InstallationOffer,
  InstallationOutcome,
} from "../../src/onboarding/application/agent-installations"
import {
  PermissionRulesProvider,
  PermissionRuleRows,
} from "../../src/desktop/settings/ui/permission-rule-rows"
import type {
  PermissionRulesGateway,
  PermissionRulesRevoke,
} from "../../src/desktop/settings/adapters/permission-rules-gateway"
import { useComposer } from "../../src/panel/ui/use-composer"
import type { useConversation } from "../../src/conversation"
import "../../src/styles.css"

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
const stale = deferred<readonly InstallationOffer[]>()
const installed = deferred<InstallationOutcome>()
const offer: InstallationOffer = { agent: "claude", archiveBytes: 1, installed: false }
let notify = () => {}
let reads = 0
function Downloads() {
  const [dispatches, setDispatches] = React.useState(0)
  const source = React.useMemo<AgentInstallations>(
    () => ({
      offers: async () => (++reads === 2 ? stale.promise : [offer]),
      subscribe: (listener) => {
        notify = listener
        return () => {
          notify = () => {}
        }
      },
      install: async () => {
        setDispatches((count) => count + 1)
        return installed.promise
      },
    }),
    [],
  )
  const reason =
    new URLSearchParams(location.search).get("reason") === "busy"
      ? "busy"
      : "not-confirmed"
  return (
    <section aria-label="Download ordering fixture">
      <AgentDownloads source={source} />
      <output aria-label="Installation requests">{dispatches}</output>
      <button onClick={() => notify()}>Refresh during install</button>
      <button onClick={() => installed.resolve({ status: "failed", reason })}>
        Finish uncertain install
      </button>
      <button onClick={() => stale.resolve([offer])}>Release earlier offers</button>
    </section>
  )
}
const first = deferred<boolean>(),
  second = deferred<boolean>()
function Composer() {
  const [id, setId] = React.useState("A")
  const [text, setText] = React.useState("Draft A")
  const submits = React.useRef(0)
  const chat = {
    active: { id, draft: text ? [{ type: "text", text }] : [] },
    submit: () => (++submits.current === 1 ? first.promise : second.promise),
    setDraft: () => {},
  } as unknown as ReturnType<typeof useConversation>
  const composer = useComposer(
    chat,
    () => false,
    () => {},
  )
  return (
    <section aria-label="Composer ordering fixture">
      <p>
        Composer tab: <output aria-label="Composer tab">{id}</output>
      </p>
      <p>
        Expanded editor:{" "}
        <output aria-label="Expanded editor">{String(composer.expanded)}</output>
      </p>
      <form onSubmit={composer.submit}>
        <button type="submit">Send draft</button>
      </form>
      <button
        onClick={() => {
          setId("B")
          setText("Draft B")
        }}
      >
        Switch to B
      </button>
      <button onClick={() => composer.changeExpanded(true, "control")}>
        Expand editor
      </button>
      <button onClick={() => setText("")}>Clear draft</button>
      <button onClick={() => first.resolve(false)}>Refuse first send</button>
    </section>
  )
}
const revoked = deferred<PermissionRulesRevoke>()
function Permissions() {
  const [requests, setRequests] = React.useState(0)
  const gateway = React.useMemo<PermissionRulesGateway>(
    () => ({
      read: async () => ({
        ok: true,
        rules: [
          {
            id: "pr_fixture",
            revision: "1",
            workspace: "/fixture",
            source: "claude",
            tool: "Bash",
            effect: "allow",
          },
        ],
      }),
      revoke: async () => {
        setRequests((count) => count + 1)
        return revoked.promise
      },
    }),
    [],
  )
  return (
    <section aria-label="Permission revoke ordering fixture">
      <PermissionRulesProvider gateway={gateway}>
        <PermissionRuleRows />
      </PermissionRulesProvider>
      <output aria-label="Revoke requests">{requests}</output>
      <button onClick={() => revoked.resolve({ ok: false })}>Finish revoke</button>
    </section>
  )
}
createRoot(document.getElementById("root")!).render(
  <main data-user-flow-ordering>
    <h1>User flow ordering</h1>
    <Downloads />
    <Composer />
    <Permissions />
  </main>,
)
