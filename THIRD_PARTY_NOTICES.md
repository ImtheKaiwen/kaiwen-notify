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

## Default notification sound (`src-tauri/resources/default.wav`)

Synthesized by `scripts/make-default-sound.py` from sine waves. It is original to this project and
released under the MIT license. Earlier releases (0.4.x) bundled a different default sound whose
origin was not documented; it was replaced for that reason.

A custom WAV you choose in the app stays on your computer and is never part of a release.

## Application icon

`src-tauri/icons/*` and `public/assets/brand/app-icon.png` are original artwork for this project (MIT).

## Software dependencies

Rust crates and npm packages keep their own licenses (mostly MIT/Apache-2.0); see `Cargo.lock` and
`package-lock.json`. Tauri is dual-licensed MIT/Apache-2.0. Microsoft Edge WebView2 is a Microsoft
runtime installed separately under its own terms.
