import { nativeAgentApiKeys } from "../onboarding/adapters/agent-api-key"
import { AgentSetupProvider } from "../onboarding"
import { assignedAgentReadiness } from "../onboarding/adapters/agents"
import type { NessaClient } from "@nessa/client"
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import {
  askReviewNoticeAccess,
  chooseChannelFolder,
  chooseChannelResources,
  homeFolder,
  host,
  loadAssignedGatewayEndpoint,
  onReviewNoticeOpened,
  reviewNoticeAccess,
  syncReviewNotices,
  sandboxSchemeRegistered,
  signInToProvider,
  providerLoginAvailable,
  saveFile,
  writeClipboardText,
} from "../host"
import { ClipboardProvider, HeldFilesProvider } from "../conversation"
import { ClockProvider } from "../clock"
import { environmentFromVite } from "../env/vite"
import { connectBrowserSession, createBrowserAuth } from "../session"
import { StartupMounted } from "../startup/ui/mounted-signal"
import { createDesktopDependencies } from "./dependencies"
import { faviconSiteIcon } from "./workspace/adapters/favicon-site-icon"
import { SiteIconProvider } from "./workspace/ui/transcript/site-icon"
import { hostGateway } from "./adapters/host-gateway"
import { workspaceBackend, type WorkspaceBackend } from "./model/workspace-backend"
import { seededWindow } from "./seeded-window"
import {
  GithubProvider,
  githubAvatar,
  LinkedDevicesProvider,
  McpServersProvider,
  NativeHooksProvider,
  PermissionRulesProvider,
} from "./settings"
import { makeDesktopStore } from "./store"
import { DesktopIconFamilyProvider } from "./ui/icons"
import { DesktopWindow } from "./ui/desktop-window"
import { platformFor, sandboxFor, WidgetRegistryProvider } from "./widgets"
import { SubagentsProvider } from "./subagents"
import {
  bindConversationTitles,
  bindNoticeAccess,
  openReviewEvent,
  showWaitingReviews,
} from "./workspace/adapters/gateway/review-notices"
import { followWorkspace, loadWorkspace } from "./workspace"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

document.documentElement.dataset.nessaModule = "started"

// Composition: the window's outside things and its widget plugins, then the
// store over them, then the tree. The store follows the workspace source for
// the window's life. Where the workspace comes from is `workspaceBackend`'s
// to say: the desktop app's own gateway, over the credential its host hands
// the panel (`adapters/host-gateway.ts`); a browser preview opened
// with `?gateway`, over the session this origin signed in to; a browser
// page whose query names a seeded run, on that in-memory workspace
// (`seeded-window.ts`); otherwise the sample (`model/workspace-backend.ts`).
const environment = environmentFromVite()
const backend = workspaceBackend(
  host.kind,
  window.location.search,
  environment.sampleWorkspace,
)
const seeded = backend === "seeded" ? seededWindow(window.location.search) : undefined
const gateway = seeded ? undefined : windowGateway(backend)

function windowGateway(
  backend: WorkspaceBackend,
): (() => Promise<NessaClient>) | undefined {
  switch (backend) {
    case "host":
      return hostGateway(environment)
    case "browser": {
      // This origin's sign-in, as the panel's browser surface keeps it.
      // Storage is reached at each use, so storage the browser blocks is
      // `createBrowserAuth`'s to survive rather than a throw before the
      // window renders.
      const auth = createBrowserAuth(window.fetch.bind(window), {
        getItem: (key) => window.sessionStorage.getItem(key),
        setItem: (key, value) => window.sessionStorage.setItem(key, value),
        removeItem: (key) => window.sessionStorage.removeItem(key),
      })
      return () =>
        connectBrowserSession({
          auth,
          stage: environment.stage,
          clientId: "nessa-browser",
          surfaceKind: "desktop",
          pageUrl: window.location.href,
        }).then((session) => session.client)
    }
    case "sample":
    case "seeded":
      return undefined
  }
}
// In the app on macOS and Linux, a waiting review is the system's notice,
// which the host shows and takes back: the webview has no notifications of
// its own there. Opening one brings this window forward on its conversation.
// Coming back to the window sends the waiting set again, so a notice the
// system refused before notifications were allowed (in System Settings, say)
// is shown.
const nativeNotices = host.kind === "macos" || host.kind === "linux"
if (nativeNotices) {
  bindNoticeAccess({
    read: reviewNoticeAccess,
    ask: askReviewNoticeAccess,
    blocked:
      "Blocked in System Settings › Notifications. Allow Nessa there to turn them on.",
  })
  void onReviewNoticeOpened((conversationId) => {
    window.dispatchEvent(openReviewEvent(conversationId))
  })
  window.addEventListener("focus", showWaitingReviews)
}
// The system's pickers for a new channel's paths, in the app on macOS and
// Windows. Linux has no picker of its own yet, and a browser none at all, so
// there a channel's folder is made for it and it takes no resources.
const channelPaths =
  backend === "host" && (host.kind === "macos" || host.kind === "other")
    ? {
        home: homeFolder,
        folder: chooseChannelFolder,
        resources: chooseChannelResources,
      }
    : undefined
const dependencies = createDesktopDependencies({
  signInToProvider,
  paths: channelPaths,
  providerLoginAvailable,
  gateway,
  reviewNotices: nativeNotices ? { sync: syncReviewNotices } : undefined,
  workspace: seeded?.workspace,
  now: seeded?.now,
  apps: {
    sandbox: sandboxFor(host.kind, document, sandboxSchemeRegistered()),
    platform: platformFor(host.kind),
  },
})
const agentSetup =
  backend === "host" && dependencies.agentInstallations
    ? {
        readiness: assignedAgentReadiness({
          endpoint: () => loadAssignedGatewayEndpoint(environment.stage),
        }),
        installations: dependencies.agentInstallations,
        apiKeys: nativeAgentApiKeys,
        signIn: signInToProvider,
        loginAvailable: providerLoginAvailable,
      }
    : undefined
const store = makeDesktopStore(dependencies)
bindConversationTitles((conversationId) => {
  const sessions = store.getState().workspace.sessions
  if (!Object.hasOwn(sessions, conversationId)) return null
  const title = sessions[conversationId].title.trim()
  return title.length > 0 ? title : null
})
store.dispatch(followWorkspace())
void store.dispatch(loadWorkspace())

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

createRoot(container).render(
  <React.StrictMode>
    <StartupMounted />
    <AgentSetupProvider source={agentSetup}>
      <Provider store={store}>
        <WidgetRegistryProvider registry={dependencies.widgets}>
          <ClockProvider now={dependencies.now}>
            <SubagentsProvider source={dependencies.subagents}>
              {/* Every icon in the window resolves through the family chosen in Settings. */}
              <DesktopIconFamilyProvider>
                <McpServersProvider gateway={dependencies.mcpServers}>
                  <LinkedDevicesProvider gateway={dependencies.linkedDevices}>
                    <ClipboardProvider write={writeClipboardText}>
                      <SiteIconProvider render={faviconSiteIcon}>
                        <HeldFilesProvider read={dependencies.heldFiles} save={saveFile}>
                          <NativeHooksProvider gateway={dependencies.nativeHooks}>
                            <PermissionRulesProvider
                              gateway={dependencies.permissionRules}
                            >
                              <GithubProvider
                                gateway={dependencies.github}
                                avatar={githubAvatar}
                              >
                                <DesktopWindow
                                  hostKind={host.kind}
                                  browserSurface={host.kind === "browser"}
                                  inspectable={import.meta.env.DEV}
                                />
                              </GithubProvider>
                            </PermissionRulesProvider>
                          </NativeHooksProvider>
                        </HeldFilesProvider>
                      </SiteIconProvider>
                    </ClipboardProvider>
                  </LinkedDevicesProvider>
                </McpServersProvider>
              </DesktopIconFamilyProvider>
            </SubagentsProvider>
          </ClockProvider>
        </WidgetRegistryProvider>
      </Provider>
    </AgentSetupProvider>
  </React.StrictMode>,
)
