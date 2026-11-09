# Linked-device focus fixture qualification

At assembled source `c5cd751f4a7c75c9a2e9966436eada47de87bddd`, a full
`pnpm frontend:check` reached 5,305 passing unit tests and one failure:

```text
FAIL src/desktop/settings/ui/linked-devices-tab.test.tsx
Linked devices > focus waits on the row while the read after a cancel is out, then takes Pair
line 373: expected activeElement.hasAttribute("data-pairing-row") to be true; received false
Test Files: 1 failed | 408 passed (409)
Tests: 1 failed | 5305 passed (5306)
```

The linked-device production/test source was unchanged from the successful
earlier CI checkout. Old UI pin `3118f2d8a15f68825ecef3fe0c337368b3d7f889`
and current `39e617600ad36fd9c7a1f41691aca239af25b495` have the identical
Git tree `a8fcbefd13e2c67aced329897d705d4ed0d1f4d2`. No UI behavior
change is attributed to that pin update.

The UI's existing close-focus handler may schedule a zero-delay fallback after
Radix's close timer. The old test awaited one zero-delay timer, which does not
establish completion of that second timer. The correction observes the same
closed-dialog and exact row-focus assertions through Vitest's bounded wait.
The third read stays unresolved throughout that observation. Only afterward
does the fixture resolve it and require focus on Pair. No click, cancel or read
is retried, and no production behavior or deadline changes.

Corrected test SHA-256:
`f63c259e9b5252b4648fc301932d54c42822412ebf123d9e2e6153a812a3c226`.

The original test file passed 15/15 in an isolated diagnostic run, which is not
used to erase its full-suite failure. The corrected file separately passed
15/15. An isolated negative control omitted the production parked-focus effect:
the corrected test failed its exact row-focus assertion after the bounded wait
(one failed, 14 unselected). The production file was restored byte-for-byte:
SHA-256 `d8ac4550f135f822730f75f33cac93dffd04a0d26d05b129cb8a24c3fa007122`.

The first frontend invocation failed earlier in a worktree fixture because its
command omitted the installed Cargo directory from PATH: 455 script tests passed,
one failed with `spawnSync cargo ENOENT`, two skipped. That invocation never
reached UI tests and is not a source/test assertion repair. The next invocation
had the correct toolchain and exposed the focus failure above. Both original
outcomes are retained, independently of the subsequent corrected qualification.
