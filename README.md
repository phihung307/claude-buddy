# Claude Buddy

**English** | [Tiếng Việt](README.vi.md)

A small character that floats on your desktop and shows what your Claude Code sessions are doing: working, waiting for
you, done, or failed. For Windows 10/11, with macOS in beta.

## Features

- **See your sessions at a glance:** the character shows whether Claude is working, needs you, is done, or hit an error.
  Click it to jump to that session's window.
- **Create a character from a description:** Codex draws it with all five states animated, including turning around and
  fight moves. No Codex? Use ChatGPT and drop the ZIP on the app.
- **Change it with another prompt:** "add a red hat" redraws it, and Codex remembers what it made.
- **Share characters:** one `.buddy` file, double-click to add.
- **Add your own:** WebM, MP4, PNG, APNG, GIF, WebP, JPG or SVG.
- **Token usage:** today, this week, this month and the last 14 days.
- **Stays out of your way:** hides during full screen, and clicks on it don't take focus from what you're typing in.
- **English and Vietnamese.**

## Install

Open PowerShell. No admin rights needed.

```powershell
irm https://raw.githubusercontent.com/taivt0109/claude-buddy/main/install.ps1 | iex
```

This downloads the app, adds a Start Menu shortcut, adds its hooks to Claude Code and starts it. `settings.json` is
backed up first and your other hooks stay as they are. Only Claude Code sessions opened after that show up.

Run the same command again to update. To skip the hooks, set `$env:CLAUDE_BUDDY_NO_HOOKS = "1"` first; the app then
asks when it starts.

The app isn't code-signed, so Windows SmartScreen or antivirus software may warn about it.

### macOS (beta)

macOS 11 or later, Apple silicon or Intel. Not signed by Apple yet, and still being tested.

```sh
curl -fsSL https://raw.githubusercontent.com/taivt0109/claude-buddy/main/install.sh | BUDDY_VERSION=v0.3.1 sh
```

To remove it, run `curl -fsSL https://raw.githubusercontent.com/taivt0109/claude-buddy/main/uninstall.sh | sh`.

## Uninstall

```powershell
irm https://raw.githubusercontent.com/taivt0109/claude-buddy/main/uninstall.ps1 | iex
```

This removes the app, its shortcut, its autostart entry and its hooks. Settings, usage data and added characters stay
in `%APPDATA%\claude-buddy-rs` unless you set `$env:CLAUDE_BUDDY_PURGE = "1"` first.

## Using it

| Action | Result |
|---|---|
| Hover over the character | Shows the full status of your sessions |
| Click the character | Opens or closes the session list (closes by itself after the mouse leaves) |
| Click the bubble | Opens the window of the session that most needs attention |
| Click a session row | Opens that session's terminal or IDE window |
| Drag the character | Anywhere on the screen; the position is remembered |
| Right-click the character or tray icon | Quick menu |
| Click the tray icon | Opens the main window (Characters, Usage, Settings) |
| Click **Share** on a character you added | Saves it as a `.buddy` file |
| Double-click a `.buddy` file, or drag it or a ZIP onto the main window | Adds that character |

Closing the main window keeps the app running in the tray. Quit from the right-click menu.

The interface is in English and Vietnamese. It follows Windows by default; change it under Settings > Language.

## Stays out of your way

- Bubbles show for a few seconds when a status changes, just wide enough for their text. Only sessions that need you or
  failed stay up.
- The tray icon has a dot colored by status, so you can check at a glance with nothing on the screen.
- Transparent parts of the buddy let the mouse through, and clicking the buddy doesn't take focus from what you're
  typing in.
- Hides during full-screen presentations, movies and games, then comes back.
- No sound or notification during full screen or Focus Assist, or when you're already in that session's window.
- The character comes in three sizes. All of this can be changed in Settings.

## Usage

Today, this week (from Monday), this month and the last 14 days, in local time. Numbers are counted from the transcripts
in `%USERPROFILE%\.claude\projects` (subagents included), reading only the `usage` numbers and never the conversation.
The big number is input + output + cache writes; cache reads are shown separately. Claude used on other machines or on
the web isn't included.

## Creating a character with Codex

If [Codex](https://openai.com/codex/) is installed and signed in on this PC, click **Create with Codex** on the Characters
page and describe the character you want. It takes about 6 to 10 minutes and uses your Codex quota. You get all five states
as animations, and you can keep working in the meantime as long as the app's window stays open. Codex only draws: the app
checks and cuts the images itself.

To change it later, select it and click **Edit with Codex…**, then say what should change, such as "add a red hat". Codex
continues the conversation that drew it and redraws all five states. The character only changes once they're all done.

No Codex? Type your description and click **Copy prompt for ChatGPT**, paste it into ChatGPT, then drag the ZIP it gives you
onto the app window.

## Sharing characters

Select a character you added and click **Share** to save it as one `.buddy` file, then send that file to anyone. To add
a character someone sent you, double-click the file or drag it onto the app window, with no restart. If a character with
that name already exists, both are kept.

A `.buddy` file only holds images, video, sound and a description. Nothing in it is ever run, and files with other
content are refused.

## Adding your own characters

Add a character from files, or import a folder (`character.json` and one media
file per state):

| Format | Notes |
|---|---|
| WebM VP9 with alpha | Transparent background |
| MP4 | No transparent background |
| PNG, APNG, GIF, WebP, JPG | White backgrounds can be removed when adding |
| SVG | Drawn as a still image; effects in the file don't play |

Custom sounds: `sounds/done` and `sounds/waiting` as `.wav` or `.mp3`. Very long clips are cut short (limit 50 MB per
file).

## Configuration

`%APPDATA%\claude-buddy-rs\config.json`, key `port` (default 47329). After changing the port, reconnect to Claude Code.

## Local API

Listens on `127.0.0.1` only. Requests with an unknown `Origin` or `Host` header are refused, so web pages can't control
the app.

- `POST /hook`: raw Claude Code hook data
- `POST /event`: events from other tools, for example `{"session_id":"a","state":"working","step":"test","label":"Backend"}`
- `GET /sessions`, `GET /usage`, `GET /health`

## Build from source

```
cargo build --release
target\release\claude-buddy-rs.exe
```

Needs Rust and the Windows SDK (to embed the icon). On first run the app asks whether to connect to Claude Code. If you
agree, it adds its own hooks to `%USERPROFILE%\.claude\settings.json`, keeping a backup as
`settings.json.buddy-rs-backup-*`.

The hooks store the full path to the exe. If you move the exe, reconnect in Settings. If you delete it by hand, turn off
"Connect to Claude Code" first (or run `--remove-hooks`), otherwise Claude Code reports hook errors.

## Development

```
cargo test
set PREVIEW_DIR=C:\some\folder && cargo test preview -- --ignored
cargo test export_app_icon -- --ignored
```

The second command exports PNGs of the characters and windows. The third regenerates `assets/app.ico` from the logo drawn
in code.

Environment variables for testing without touching real data: `CLAUDE_BUDDY_RS_HOME` (separate data folder, can run side
by side), `CLAUDE_CONFIG_DIR` (Claude folder), `CLAUDE_BUDDY_RS_THEME=dark|light`, `CLAUDE_BUDDY_RS_BACKDROP=blur`,
`CLAUDE_BUDDY_RS_KEEP_ANIMATING=1` (keeps animating while locked, for measuring CPU).

## License

MIT, see [LICENSE](LICENSE).
