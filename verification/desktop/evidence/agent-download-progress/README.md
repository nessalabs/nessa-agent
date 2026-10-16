# Agent download progress (#942)

Source base: `3197cb6e4f769ecd78ebfd2d2564c7d2df0c7d7c` plus this PR's
onboarding UI, fixture, checks and module-map changes. Screenshots are the real
setup/download-list components over a controlled installation port, at 360px.
They contain fixture data, not an owner account.

- `pnpm frontend:check`: format, lint, architecture, protocol, client docs/types,
  application types, 352 frontend test files / 4,645 tests and production build
  completed. Supporting scripts: 425 total, 423 passed, two existing skips;
  desktop-library tests 200 passed; architecture tests 104 passed.
- Focused progress/setup tests: 27 passed.
- `send-failure.mjs`: 15/15 held, Chromium and WebKit, setup, first-use and
  downloads at 360px; no numeric completion value, known archive size,
  visible animated indicator, reduced-motion stop, no replay, terminal removal.
- `pnpm test:e2e:scripted`: passed all three gateway checks in both engines;
  [summary](scripted-summary.md).
- Five source mutations were verified applied and failed their checks: invented
  numeric completion, wrong known archive size, invented unknown archive size,
  missing animation, ignored reduced motion. Source was restored with a fresh
  modification time and the targeted browser suite passed again.
- Fresh FULL review round 2 found no findings at any priority. Round 1 found
  native progress painting as a solid rectangle; the shared component now uses
  the existing reduced-motion-aware spinner and its browser check observes a
  changing transform.

The complete functional desktop run remains in progress; this evidence does
not claim that run passed. There is no new performance-budget scenario.

![WebKit setup](webkit-setup.png)
![Chromium downloads](chromium-downloads.png)
