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
 * ```
 *
 * An arrow points from what reads to what it reads. The window mounts
 * `SettingsHost` once; anything opens it with `openSettings`. Composition
 * (`dependencies.ts`) builds the servers' gateway over the window's client
 * and `main.tsx` provides it (`McpServersProvider`); with none, Integrations
 * is pending.
 */
export { openSettings, SettingsHost, useSettingsOpening } from "./ui/settings-view"
export { McpServersProvider } from "./ui/integrations-tab"
export {
  mcpServersGateway,
  type McpServersClient,
  type McpServersGateway,
} from "./adapters/mcp-servers-gateway"
