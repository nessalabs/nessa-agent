# Thread tools and current-main integration verification

The assembled production source is 302967fe (API b1ef2e3d, source-admission fix
11d4f1b1, main 10907952). The additional changes in this commit update browser
drivers for main #954's completed-tool folding; product behavior and permissions
are unchanged.

The first final scripted run failed in Chromium and WebKit: the MCP app frame
was present, but its completed tool step was hidden behind “Ran 1 tool”; the
scenario's completed “Report the rows” step was hidden the same way. The saved
screenshots and unchanged strict step-count/title assertions distinguished this
from missing app content or gateway events. Gateway-window passed 13/13.

Both checks now use the workspace driver to open completed tool work before
inspecting its individual steps. Its CSS stays in the shared selector owner.
The driver uses stable toggle locators and only clicks collapsed runs. A real
Chromium probe verified two collapsed runs open, an already open run remains
open, and repeated inspection performs no additional clicks. The module map
describes this responsibility. No assertion or timeout was weakened.

The first full corrected run and the final stable-toggle version passed all
three checks in both Chromium and WebKit: MCP apps 16/16, gateway-window 13/13,
scripted scenarios 8/8. Five existing driver/selector tests, syntax checks,
formatting and lint pass.

Production-source checks pass: nine agent-tool tests; the four-case source race
regression; a late-reload revert probe (broken 101, restored 0); all 67 normal
product-protocol checks; seven-package strict Clippy; Rust formatting, client
type checking, repository formatting, architecture and SDK documentation.

Local evidence is saved under `/workspace/research/950-*`; the final browser
summary is `950-scripted-e2e-final-work/pr-summary.md`. Hosted exact-head checks,
fresh clean Codex reviews and guarded sequential integration remain required.
