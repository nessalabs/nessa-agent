# PR 739 image readback

At pushed head 05bc3c6c3b1ab1a9148bfc45f7cb133cc6f855fe, the owned hidden GitHub IAB tab independently read four actual image elements: two PR-body screenshots and two existing-comment screenshots. All four were complete with natural width 440 and height 480. Their source URLs were GitHub immutable 5c1c2257c630a3f5d5dc83ba422c882ecc598fbd PNG paths; the source images are unchanged at 05bc. Earlier source screenshots were visually inspected. Accessibility representation wrapped these images as links, while decoded natural dimensions confirmed actual image loading. All five existing Mermaid diagrams were previously rendered and inspected at PR 739, recorded in pr-739-render-verification.md; body edits preserve their exact source.

The sole coordinator IAB tab was closed before final whole verification was released. No screenshot or page content was modified, and no user browser tab was touched.
