# GitHub credential containment evidence

The connected row uses the fake GitHub account in the conversation stress fixture:

![Connected GitHub account](github-connected.png)

Checks on `codex/1190-github-token` in the isolated checkout:

```text
conversation-stress: 53/53 held
ok    042 github [chromium columns]
conversation-stress: 6/6 held    # two GitHub cycles, Chromium
conversation-stress: 5/5 held    # two GitHub cycles, WebKit
mcp-apps-gateway: 16/16 held
gateway-window: 15/15 held
scripted-scenarios: 9/9 held
Verdict: pass
```

The two-cycle stress run asserts that neither an agent process nor the managed shell receives Nessa's `GH_TOKEN` or git credential helper, and that the broker makes one authorized repository read per cycle. The full default run also reaches Connect, Disconnect, and the remaining conversation steps. The scripted suite covers Chromium and WebKit. The standalone image is a crop of the Chromium connected-state screenshot; the fake account has no real credential.

Revert probes: allowing `GH_TOKEN` into the managed shell made `commands_drop_the_github_sign_in_and_other_credentials` fail with `left: ["PATH", "GH_TOKEN"]`; allowing `/` in a repository segment made `repository_paths_have_only_fixed_segments_and_positive_numbers` fail on `repo/other`. Both rules were restored and their focused tests passed.
