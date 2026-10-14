# Windows replacement investigation (#874)

Checked fixed source: `70c0a025b5d39a0b55766eb2cc8d12540c57082c`.
The Mac real-gateway runner rebuilt the server from this source with its explicit
private target and supplied that binary to all checks. MCP apps 16/16, gateway
window 13/13, scripted scenarios 8/8 passed in Chromium and WebKit.

The native baseline failure is attributed separately to `57463764`, Windows
job 116023819318: an ordinary private delete-sharing reader reproduces OS 5 under
legacy replacement, while blocked-sharing and readonly controls pass. This proves
the replacement defect; it does not identify the original failed CI handle owner.
Fixed-head native CI was running when these artifacts were saved and must be
verified separately before merge. Mac browser execution does not prove Windows
handle semantics. The one recorded aborted browser check after a 204 is the
existing documented harmless condition; no failure or budget was suppressed.
