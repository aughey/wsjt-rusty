# README screenshots

Captured on 2026-11-22 from a local release build of the browser app using desktop
Chrome at 1440 pixels wide. `conversations.png` uses a 1080-pixel viewport height;
`stations.png` uses 1500 pixels to include the queued station cards.

Input is the unchanged [FT8 fixture](../../tests/fixtures/ft8-210703-133430.wav),
fed through Chromium's simulated microphone into the actual AudioWorklet and
Rust/WASM decoder. Messages and waterfall data are decoded from that recording,
not injected UI events. Displayed UTC dates belong to the capture session, not
the original recording. `W9XYZ` / `EN37` are example operator settings.

To recreate similar views, build with `trunk build --release` from `crates/web`
into an isolated output directory and serve it on localhost. Launch Chrome with
`--use-fake-device-for-media-stream`, `--use-fake-ui-for-media-stream`, and
`--use-file-for-fake-audio-capture=ABSOLUTE_PATH_TO_FIXTURE`. Start listening and
allow about 45 seconds for slot and recovery decodes. Capture Conversations,
select three cards, then capture Stations to contact. Keep contacting paused;
these screenshots use neither a connected radio nor transmit audio. Stop
listening afterward to flush the decoders and release input.

The map uses OpenStreetMap tiles with attribution visible in the screenshot.
