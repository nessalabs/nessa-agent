/**
 * Settings: its own surface over the window, opened from the identity at the
 * foot of the sidebar or with ⌘,. See ADR 238.
 *
 * ```text
 *   model/settings-catalogue.ts ── categories → tabs → settings, as data;
 *        │                         the one owner of every name, description
 *        │                         and what search finds
 *        ▼
 *   ui/settings-view.tsx ──▶ ui/settings-tabs.tsx ──▶ ui/settings-controls.tsx
 *   (the surface: sidebar,    (each tab's rows, wired   (grouped rows and the
 *    tabs, search, focus)      to the preferences and    quiet controls a row
 *                              the workspace it sets)    holds)
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
 * ```
 *
 * An arrow points from what reads to what it reads. The window mounts
 * `SettingsHost` once; anything opens it with `openSettings`. Composition
 * (`dependencies.ts`) builds the servers' gateway and the pairing gateway
 * over the window's client and `main.tsx` provides them; with none, those
 * tabs are pending.
 */
export { openSettings, SettingsHost, useSettingsOpening } from "./ui/settings-view"
export { McpServersProvider } from "./ui/integrations-tab"
export { LinkedDevicesProvider } from "./ui/linked-devices-tab"
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
