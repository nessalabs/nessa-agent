# Sidebar row hover (#1325)

Source fix: `ac0e8a86b140b1577552a4a5de596e158444c6b2`, based on
`edf3e028bf48748147ad2a0a35d028e2fc7cfca1`.

The kit places a row's actions beside its control. Pointing at that sibling
keeps the frame hovered, but stops hovering the control. The frame now supplies
the control's hover fill. Selected/pressed precedence and action targets stay
with their existing rules. The selector has the same specificity `(0,2,0)`.

Chromium before / after (1440 × 900, dev build, cropped to the same row):

![Before](before-chromium.png)
![After](after-chromium.png)

WebKit before / after:

![Before](before-webkit.png)
![After](after-webkit.png)

The existing measured regression was run without changing its assertions or
bounds, in Chromium and WebKit, columns and sidebar:

```sh
node verification/desktop/scripts/shared-controls.mjs --only rows \
  --engine chromium,webkit --layout columns,sidebar --channel bundled
```

Fixed: `shared-controls: 4/4 held`. Reverting only the selector: `2/4 held`;
both engines' columns fail with `kit row under its glyph: its fill is
rgba(0, 0, 0, 0)`. Restoring: `shared-controls: 4/4 held`. Both sidebar cases
also hold throughout. The same pinned frame retains pointer membership and
its action hit target in both engine screenshots. The rows check also measures
selected/unread/name state, count/glyph hover, action visibility and hits.

Documentation review with the nessa-docs skill: no state page changes or
checkpoint advances. This selector changes a transient paint predicate; it
changes no stored state, command, admission, lifecycle or flow described by
`docs/state`. The existing shared-controls checklist already owns this exact
hover/action contract. No unrelated workspace page was certified.

Independent source review found no implementation findings. Browser execution
supports only the focused claim above. Full desktop qualification is not green:
smoke reports `28/32 held` because its existing Advanced settings explanation
assertion fails in all four profiles. This change must remain draft until
applicable browser and other repository gates pass. Native WKWebView/manual
interaction, the final full functional matrix and the scripted gateway flow
have not been rerun for this isolated repair.
