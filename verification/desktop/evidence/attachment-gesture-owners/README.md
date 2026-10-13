# Attachment gesture owner evidence

Issue #824. Product source was frozen at `7870e750b7483772ebe4ba1a66546660493ef568`,
against #816 source `22f76f61886b05cc598b37ddf3fb3acd9ce124e5`.
The later evidence commit adds these files only.

The actual panel fixture holds two native picker calls across Redux providers
with the same local `c0`, announces their readiness in reverse order, and
asserts the visible provider shows only its own file. It then switches back,
checks the original provider's tile, resolves both results in reverse order,
and checks each store's draft. These cropped screenshots show provider B's
own `b.pdf` tile. They are supplemental visual evidence; the assertions prove
routing and cleanup.

![Provider B file, Chromium](picker-owner-chromium.png)
![Provider B file, WebKit](picker-owner-webkit.png)

[check.log](check.log) records only the short gate and revert results. The full
source ordering table and public regressions live in
[attachment-gestures](../../../../docs/design/attachment-gestures.md).
These controlled host tests do not establish native platform support or server
authorization; current-main assembly and CI remain separate gates.
