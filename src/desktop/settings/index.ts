/**
 * Settings: its own surface over the window, opened from the identity at the
 * foot of the sidebar or with ⌘,. See ADR 238.
 *
 * ```text
 *   model/settings-catalogue.ts ── categories → tabs → settings, as data;
 *        │                         the one owner of every name, description
 *        │                         and what search finds
 *        ▼
 *   ui/settings-view.tsx ──▶ ui/use-settings-surface.ts ──▶ ui/settings-layout.tsx
 *   (open, close, ⌘,)        (sidebar, chords, place,     (the markup)
 *                             focus, scroll)
 *        │
 *        ▼
 *   ui/settings-tabs.tsx ──▶ ui/settings-controls.tsx
 *   (each tab's rows, wired   (Group, Row, ItemRow,
 *    to the preferences)       over the UI kit's)
 *        │
 *        ▼
 *   ui/settings-masthead.tsx
 *   (a page's title, its one line, its tabs)
 *                                    │
 *                                    ▼
 *   ui/integrations-tab.tsx ──▶ model/mcp-servers.ts ◀── adapters/mcp-servers-gateway.ts
 *   (Integrations: the          (its states and rules,   (`client.mcpServers` read
 *    gateway's MCP servers,      the design's U1–U43)     into the model's words)
 *    drawn and sent)
 *
 *   ui/linked-devices-tab.tsx ──▶ model/linked-devices.ts ◀── adapters/linked-devices-gateway.ts
 *   (Linked devices: pair,        (its states and rules,    (`client.pairing`, credentials
 *    approve, revoke; the UI      rows L1–L21)              and the session, read into
 *    kit's code, orbs and                                    the model's words; the key
 *    fingerprint)                                            hash is model/device-key.ts)
 *
 *   ui/native-hooks-rows.tsx ──▶ adapters/native-hooks-gateway.ts
 *   (Native hooks: Claude/Codex   (`client.agents` list and
 *    hook switches)                setNativeHooks)
 *
 *   ui/permission-rule-rows.tsx ──▶ adapters/permission-rules-gateway.ts
 *   (Access: remembered rules     (`client.conversation` list and
 *    and revoke)                   revokePermissionRule)
 *
 *   ui/github-row.tsx ──▶ model/github.ts ◀── adapters/github-gateway.ts
 *   (Accounts: connect      (its views)          (`client.github` read into
 *    through the Nessa                            the model's words, with a
 *    GitHub App's code)                           sentence for each code)
 *          ▲
 *          └── adapters/github-avatar.tsx (the account's picture from
 *              GitHub, composed by `main.tsx`; the row alone draws an initial)
 *
 *   ui/review-notice-rows.tsx
 *   (Access: the button that asks the browser to allow a notification
 *    while a review waits)
 * ```
 *
 * An arrow points from what reads to what it reads. The window mounts
 * `SettingsHost` once; anything opens it with `openSettings`. Composition
 * (`dependencies.ts`) builds the servers' gateway, the pairing gateway, and
 * the native-hooks gateway and the GitHub gateway over the window's client and `main.tsx` provides
 * them; with none, those tabs are pending and the hook rows rest.
 */
export { openSettings, SettingsHost, useSettingsOpening } from "./ui/settings-view"
export { McpServersProvider } from "./ui/integrations-tab"
export { LinkedDevicesProvider } from "./ui/linked-devices-tab"
export { GithubProvider } from "./ui/github-row"
export { VoiceKeyProvider } from "./ui/voice-key-row"
export { NativeHooksProvider } from "./ui/native-hooks-rows"
export { PermissionRulesProvider } from "./ui/permission-rule-rows"
export {
  mcpServersGateway,
  type McpServersClient,
  type McpServersGateway,
} from "./adapters/mcp-servers-gateway"
export {
  linkedDevicesGateway,
  type LinkedDevicesClient,
  type LinkedDevicesGateway,
} from "./adapters/linked-devices-gateway"
export {
  nativeHooksGateway,
  type NativeHooksClient,
  type NativeHooksGateway,
} from "./adapters/native-hooks-gateway"
export {
  githubGateway,
  type GithubClient,
  type GithubGateway,
} from "./adapters/github-gateway"
export { githubAvatar } from "./adapters/github-avatar"
export {
  permissionRulesGateway,
  type PermissionRulesClient,
  type PermissionRulesGateway,
} from "./adapters/permission-rules-gateway"
