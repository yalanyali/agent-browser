# Ardia

Ardia is a Chromium browser that the user runs as their main browser, signed in to everything. `--ardia` attaches to it through the control file Ardia writes (`~/Library/Application Support/com.ardia.browser/Chromium/ArdiaControl.json`, or `$ARDIA_DATA_DIR/Chromium/ArdiaControl.json`).

## Start

```bash
agent-browser --session work --ardia open https://example.com
agent-browser --session work snapshot -i     # later commands stay on Ardia
```

`connect ardia` is equivalent to `--ardia`. The session stays attached until `close`, which only disconnects and leaves Ardia and its tabs open.

## Rules the session follows

- **Own tab.** The session opens a background tab of its own and works there. It never adopts the user's current or first tab, and a tab the user opens never takes over the session.
- **No focus changes.** No `Page.bringToFront`, new tabs open in the background, and a tab Ardia has parked is reported as parked (`Ardia has parked this tab`) instead of being activated. `--front` allows focus changes; use it only when the user asks to see the page.
- **User tabs only on request.** `tab list` shows the user's tabs. Switch to one with `tab <ref>` only when the user asks you to work on it. The session stays on that tab until `tab new` or another `tab <ref>`. Switching never brings the tab to the front.

## Not available on Ardia

Ardia enforces a CDP allowlist. These commands fail immediately with `not available on Ardia`: `cookies`, `state save`, `state load`, `set viewport`, `set device`, `set media`, `set geo`, user agent, timezone and locale overrides, `set headers`, `set credentials`, `set offline`, `network route`/`unroute`, `upload`, `download`, `wait --download`, `trace`, `profiler`, `record`, `stream enable`, `window new`, and WebMCP. `--allowed-domains` is rejected.

Everything else works: navigation, snapshots, clicks, typing, keyboard and mouse input, `eval`, screenshots, PDFs, network request inspection and tabs.
