# Huddle spike

Throwaway code for one question: can an MCP App inside Nessa hold a realtime
voice call with OpenAI, and show and mark files while it talks? The design is
in the project's "Nessa Huddle: design" doc. The real Huddle server will live
in its own repo and reach the gateway as a client. This one only reads files
under one folder.

- `server.mjs`: a stdio MCP server, no dependencies. `huddle_open` shows the
  app. The app calls `huddle_start_call` (mints a 60-second OpenAI client
  secret), `huddle_connect` (the fallback: the server sends the call offer with
  the real key), `huddle_read_file` and `huddle_list_files`.
- `app.html`: the app. A WebRTC call to `gpt-realtime`, the stage tools
  `show`, `draw`, `view` and `clear`, interruption with Esc, push to talk with
  Space, captions, and the sand avatar driven by the live audio level.

## Try it in Nessa on a Mac

1. Build and run Nessa from this branch (the host now passes the microphone to
   an app that asks for it, and the app bundle asks macOS for the microphone).
2. Settings, MCP servers, add a stdio server named `huddle`:
   command `node`, argument `<repo>/experiments/huddle/server.mjs`, and
   environment `OPENAI_API_KEY` (your key) and `HUDDLE_ROOT` (the folder to
   talk about). Optional: `HUDDLE_MODEL`, `HUDDLE_VOICE`, and
   `HUDDLE_KEY_FROM_PROXY=1` where a proxy adds the key to requests to
   api.openai.com (a cloud sandbox's network secret), so no key is set here.
3. In a new chat, ask the agent to "open a huddle on README.md".
4. Press Start. The line at the bottom right says whether the host granted the
   microphone and which route connected the call.

## What the spike found so far

In Chromium, `getUserMedia` refuses a frame with an opaque origin
("SecurityError: Invalid security origin"), whatever its `allow` attribute
says, and Nessa runs every app in an opaque-origin frame by design. The same
frame given a real origin gets the microphone. So an app needs an origin of
its own before it can listen, as ADR 344 already says ("an origin per server
is the change that would allow more"). Whether WebKit behaves the same is the
first thing to check on the Mac.
