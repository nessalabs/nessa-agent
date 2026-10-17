# Parent Stop browser evidence

The two screenshots show the ordinary parent transcript after Stop, with the
cancelled turn and a usable reply composer, for the default and explicit
`subagents.stopChildrenOnParentStop: false` settings.

Captured in Chromium production mode at runtime commit `ffec1c29758d7f79d92d438c653d6426ef1dce0a`.
The gateway used temporary verification configuration; live configuration and
provider credentials were not changed. The corresponding scripted run passed
all 41 canonical checks in Chromium and WebKit, and the four focused parent Stop
checks in both engines using the Codex capability profile.

The pull request records the final source head, browser binary hash and rerun
results after subsequent receipt-access and report-continuation corrections.
These screenshots illustrate the surface; the scripted assertions prove the
recursive default, explicit override, unrelated execution preservation and
report delivery after later person input.
