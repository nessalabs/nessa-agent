import { useCallback, useEffect, useState } from "react"
import { NessaRpcError } from "@nessa/client"
import { isAuthenticationFailure } from "../session/adapters/client/authentication-failure"
import { Provider } from "react-redux"
import { conversationTabSnapshot, restoreConversations } from "../conversation"
import { createTabStorage } from "../conversation/adapters/browser/tab-storage"
import { SetupGate } from "../onboarding"
import { App } from "../panel"
import {
  canUseGateway,
  SessionLifecycle,
  BrowserSignIn,
  signOutBrowserSession,
} from "../session"
import {
  createBrowserAuth,
  browserSessionUrl,
} from "../session/adapters/client/browser-auth"
import { connectDevSession } from "../session/adapters/client/dev-session"
import { makeStore } from "../store"
import type { Environment } from "../env/environment"
import { maintainBrowserSession } from "../session/adapters/lifecycle/browser-renewal"
import { createDependencies } from "./dependencies"

function createScope(
  environment: Environment,
  auth: ReturnType<typeof createBrowserAuth>,
) {
  const dependencies = createDependencies({
    environment,
    connectSession: async () => {
      if (!(await auth.restore()))
        throw new NessaRpcError("unauthorized", "Please sign in again.")
      return connectDevSession({
        stage: environment.stage,
        clientId: "nessa-browser",
        browserUrl: browserSessionUrl(window.location.href, environment.stage),
      })
    },
  })
  const store = makeStore(dependencies)
  const tabStorage = createTabStorage({
    getItem: (key) => window.localStorage.getItem(key),
    setItem: (key, value) => window.localStorage.setItem(key, value),
  })
  let ownerKey = ""
  let savedText = ""
  const unsubscribe = store.subscribe(() => {
    const state = store.getState()
    if (!canUseGateway(state.session)) return
    const owner = state.session.hello
    const nextKey = JSON.stringify([
      owner.gatewayId,
      owner.organizationId,
      owner.principalId,
    ])
    if (ownerKey !== nextKey) {
      ownerKey = nextKey
      savedText = ""
      const saved = tabStorage.read(owner)
      if (saved) {
        store.dispatch(restoreConversations(saved))
        return
      }
    }
    const refs = conversationTabSnapshot(state.conversation)
    const text = JSON.stringify(refs)
    if (text !== savedText) {
      savedText = text
      tabStorage.write(owner, refs)
    }
  })
  return {
    dependencies,
    store,
    dispose: () => {
      unsubscribe()
      dependencies.session.get()?.close()
      dependencies.attachments.retain(new Set())
    },
  }
}
type Scope = ReturnType<typeof createScope>

function BrowserSession({
  scope,
  error,
  onDisconnect,
  onTerminalFailure,
}: {
  scope: Scope
  error?: string
  onDisconnect: () => void
  onTerminalFailure: (error: unknown) => void
}) {
  return (
    // The choice is carried in place: this surface has no host to write it to,
    // so the gate telling the dependencies is the only record it gets.
    <SetupGate
      agents={scope.dependencies.agents}
      onHandOver={scope.dependencies.rememberChosenAgent}
    >
      <SessionLifecycle
        dependencies={scope.dependencies}
        onTerminalFailure={onTerminalFailure}
      />
      <App
        attachmentResources={scope.dependencies.attachments}
        onSignOut={onDisconnect}
        sessionError={error}
      />
    </SetupGate>
  )
}

/** Browser composition owns the cookie session's transport and store, never its secret. */
export function BrowserApplication({ environment }: { environment: Environment }) {
  const [auth] = useState(() =>
    createBrowserAuth(window.fetch.bind(window), {
      getItem: (key) => window.sessionStorage.getItem(key),
      setItem: (key, value) => window.sessionStorage.setItem(key, value),
      removeItem: (key) => window.sessionStorage.removeItem(key),
    }),
  )
  const [scope, setScope] = useState<Scope | null>(null)
  const [busy, setBusy] = useState(true)
  const [error, setError] = useState<string>()
  const [retry, setRetry] = useState(0)
  useEffect(() => {
    let disposed = false
    setBusy(true)
    setError(undefined)
    try {
      browserSessionUrl(window.location.href, environment.stage)
    } catch (cause) {
      setError((cause as Error).message)
      setBusy(false)
      return
    }
    void auth
      .restore()
      .then((signedIn) => {
        if (!disposed && signedIn) setScope(createScope(environment, auth))
      })
      .catch((cause: unknown) => {
        if (!disposed)
          setError(cause instanceof Error ? cause.message : "Unable to restore sign-in.")
      })
      .finally(() => {
        if (!disposed) setBusy(false)
      })
    return () => {
      disposed = true
    }
  }, [auth, environment, retry])
  const discardScope = useCallback(() => {
    scope?.dispose()
    setScope(null)
  }, [scope])
  const onTerminalFailure = useCallback(
    (cause: unknown) => {
      if (isAuthenticationFailure(cause)) {
        discardScope()
        setError("Your session ended. Please sign in again.")
      }
    },
    [discardScope],
  )
  useEffect(() => {
    if (!scope) return
    return maintainBrowserSession({
      check: auth.restore,
      ended: () => onTerminalFailure(new NessaRpcError("unauthorized", "Session ended")),
      visible: () => document.visibilityState === "visible",
      subscribe: (check) => {
        document.addEventListener("visibilitychange", check)
        window.addEventListener("focus", check)
        return () => {
          document.removeEventListener("visibilitychange", check)
          window.removeEventListener("focus", check)
        }
      },
      schedule: (check) => {
        const timer = window.setInterval(check, 5 * 60 * 1000)
        return () => window.clearInterval(timer)
      },
    })
  }, [scope, auth, onTerminalFailure])
  async function disconnect() {
    setBusy(true)
    setError(undefined)
    try {
      await signOutBrowserSession(discardScope, auth.logout)
    } catch (cause) {
      setError(
        cause instanceof Error
          ? cause.message
          : "Server sign-out could not be confirmed.",
      )
    } finally {
      setBusy(false)
    }
  }
  async function connect(token: string) {
    setBusy(true)
    setError(undefined)
    try {
      browserSessionUrl(window.location.href, environment.stage)
      await auth.login(token)
      setScope(createScope(environment, auth))
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Unable to sign in.")
    } finally {
      setBusy(false)
    }
  }
  if (!scope)
    return (
      <BrowserSignIn
        busy={busy}
        error={error}
        onConnect={connect}
        onRetry={error ? () => setRetry((value) => value + 1) : undefined}
      />
    )
  return (
    <Provider store={scope.store}>
      <BrowserSession
        scope={scope}
        error={error}
        onDisconnect={disconnect}
        onTerminalFailure={onTerminalFailure}
      />
    </Provider>
  )
}
