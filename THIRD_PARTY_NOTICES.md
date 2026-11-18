# Third-party notices

The MIT license in [LICENSE](LICENSE) covers Kaiwen Notify's own source code only. It does **not**
relicense the items below.

## Provider logos (`public/assets/providers/`)

| File | Represents | Rights holder |
|---|---|---|
| `claude-ai.svg` | Claude / Claude Code | Anthropic |
| `openai-chatgpt.svg` | OpenAI / ChatGPT, used for Codex | OpenAI |
| `antigravity.png` | Google Antigravity | Google |

These are trademarks or brand assets of their owners. They are shown only to identify the tools
Kaiwen Notify integrates with, which does not imply endorsement. They are not MIT-licensed. If an
owner objects or publishes brand rules that this use does not meet, remove the file; the UI falls
back to a letter mark automatically. The origin and exact license terms of these files were not
documented when they were added; the maintainer should confirm them.

## Default notification sounds (`src-tauri/resources/default.wav`, `subagent.wav`)

Synthesized by `scripts/make-default-sound.py` from sine waves: a two-note chime for finished tasks and a single
softer note for finished sub-agents. Both are original to this project and released under the MIT license. Earlier
releases (0.4.x) bundled a different default sound whose origin was undocumented; it was replaced for that reason.

Test audio (`src-tauri/test-fixtures/tone.wav|mp3|ogg`) are 0.3 s sine tones generated for the unit tests; they are
not part of the installer.

Custom sounds you choose in the app are copied into the app's private data folder on your computer; they are never
part of a release and are not uploaded anywhere.

## Application icon

`src-tauri/icons/*` and `public/assets/brand/app-icon.png` are original artwork for this project (MIT).

## Software dependencies

Audio decoding uses Symphonia (MPL-2.0; MP3, Ogg Vorbis and WAV only), used unmodified from crates.io. Its source is available at https://github.com/pdeljanov/Symphonia and the MPL-2.0 text at https://www.mozilla.org/MPL/2.0/ ; MPL-2.0 covers only Symphonia's own files, not this application's code, which stays MIT. Rust crates and npm packages keep their own licenses (mostly MIT/Apache-2.0); see `Cargo.lock` and
`package-lock.json`. Tauri is dual-licensed MIT/Apache-2.0. Microsoft Edge WebView2 is a Microsoft
runtime installed separately under its own terms.
