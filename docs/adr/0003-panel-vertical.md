# 0003. Panel chrome is its own vertical

- **Date:** 2026-09-01
- **Status:** accepted

## Context

After the conversation vertical landed, the floating-window hooks still sat
on `src/`: colour scheme, edge reveal, panel frame, surface, host-panel
wiring, the waveform glyph. They are not product rules and they are not a
utils pile — they are the panel surface.

`src/` as a flat bag of hooks hid that. A new subscription had no obvious
home. The host seam (`host-window.ts`) sat beside them even though
`src/host/` already owned OS feature injection.

## Decision

Two more homes, same shape as conversation:

```
src/panel/
  model/       Surface
  adapters/    host subscriptions (scheme, glow, frame, frost, surface)
  ui/          chrome (`app.tsx`) and the waveform glyph

src/host/
  window.ts    the one file that may import @tauri-apps
  *.ts         injected HostFeatures
```

`src/` keeps only the composition root: `main.tsx`, `store.ts`, plus the
dev-only icon preview. The architecture check fails a new `.ts`/`.tsx` at
that root.

The panel vertical has no use-case folder. These adapters subscribe to the
OS; they do not encode product commands. When a preference earns a server
(surface remembered remotely), it gets a use case then — not before.

## Consequences

A host subscription goes in `src/panel/adapters/`. A chrome widget goes in
`src/panel/ui/`. A new Tauri command goes in `src/host/window.ts` and
`host.rs` together. The conversation vertical does not grow window code.
