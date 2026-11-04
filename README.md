# Claude Buddy

**English** | [Tiếng Việt](README.vi.md)

A small character that floats on your desktop and shows what your Claude Code sessions are doing: working, waiting for
you, done, or failed. Written in Rust on plain Win32, with no Electron and no WebView. Windows 10/11 only.

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

## Uninstall

```powershell
irm https://raw.githubusercontent.com/taivt0109/claude-buddy/main/uninstall.ps1 | iex
```

This removes the app, its shortcut, its autostart entry and its hooks. Settings, usage data and added characters stay
in `%APPDATA%\claude-buddy-rs` unless you set `$env:CLAUDE_BUDDY_PURGE = "1"` first.

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

## Why it's light

- Each animation is prepared once, in a child process that exits, so the memory used for decoding is all given back.
- During playback the app maps the cache file instead of loading it into RAM, so Windows can reclaim that memory whenever
  it needs to.
- Each frame decompresses and updates only the area that changed; if nothing changed, the app doesn't wake up.
- Video noise is filtered out during preparation, so still parts of the image aren't redrawn.
- Animation stops completely when the PC is locked, the display is off, the buddy is hidden, or a full-screen app is
  running. Still images use no timer.

## Usage

Today, this week (from Monday), this month and the last 14 days, in local time. Numbers are counted from the transcripts
in `%USERPROFILE%\.claude\projects` (subagents included), reading only the `usage` numbers and never the conversation.
The big number is input + output + cache writes; cache reads are shown separately. Counting is incremental, in a
short-lived low-priority process. Claude used on other machines or on the web isn't included.

## Characters

Every character is available right away. Add your own from files, or import a folder in the same format as the original
Electron app (`character.json` and one media file per state):

| Format | Notes |
|---|---|
| WebM VP9 with alpha | Transparent background, decoded by a Rust library built into the app |
| MP4 | Decoded by Windows Media Foundation, no transparent background |
| PNG, APNG, GIF, WebP, JPG | White backgrounds can be removed when adding |
| SVG | Drawn as a still image; effects in the file don't play |

Custom sounds: `sounds/done` and `sounds/waiting` as `.wav` or `.mp3`. Each file is decoded once and stored compressed in
`cache`, so replaying it is very light. Very long clips are cut during preparation (limit 50 MB per file).

## Configuration

`%APPDATA%\claude-buddy-rs\config.json`, key `port` (default 47329, different from the Electron app's 47321, so both can
run side by side). After changing the port, reconnect to Claude Code.

## Local API

Listens on `127.0.0.1` only. Requests with an unknown `Origin` or `Host` header are refused, so web pages can't control
the app.

- `POST /hook`: raw Claude Code hook data
- `POST /event`: events from other tools, for example `{"session_id":"a","state":"working","step":"test","label":"Backend"}`
- `GET /sessions`, `GET /usage`, `GET /health`

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
