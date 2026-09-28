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
 * ```
 *
 * An arrow points from what reads to what it reads. The window mounts
 * `SettingsHost` once; anything opens it with `openSettings`.
 */
export { openSettings, SettingsHost, useSettingsOpening } from "./ui/settings-view"
