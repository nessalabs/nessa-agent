# Prepared submission verification

Captured from reviewed source `ce5ab3c02d9e989ec118854ab3312630a07d3ef4`,
based on `9e00ba00050820fbd29293859406e3d5bcca6e59`. The normal signed-out
`scripted-e2e.mjs --channel bundled` runner passed all three checks in Chromium
and WebKit using the verified baseline gateway binary and an independent Cargo
target directory.

- `permission-card-chromium.png`: scripted pending permission card, Chromium.
- `cancelled-transcript-webkit.png`: retained transcript after the gateway
  confirmed cancellation, WebKit. The scenario asserts the gateway turn status;
  the screenshot shows its retained text.

These are pixel crops of the runner's actual screenshots. Original captures and
full logs are local verification artifacts; short result lines belong on the PR.
The files are evidence only and do not change product source.
