# AuthBaton

English | [简体中文](README.zh-CN.md)

A macOS menu-bar app that manages several Codex (ChatGPT) and Claude Code subscription accounts: sign-in, token renewal, quota and switching. You keep using the official clients; AuthBaton only handles the accounts and their credentials. When an account runs low, AuthBaton can use a reset or pass the baton to another account, so your work keeps going.

The interface is in Chinese.

> [!NOTE]
> AuthBaton was developed under the name AuthManager. The app bundle file (`AuthManager.app`), its data directory, the source folders and the integration names still use that name.

> [!IMPORTANT]
> This is an unofficial project, not affiliated with OpenAI or Anthropic. It relies on the official clients' local storage formats and undocumented interfaces, so it may stop working when they update. Check for yourself whether your use complies with each service's terms.

## Features

- **Accounts**: sign in new accounts in an isolated directory, take over the account you are already signed in to, or import an existing Codex `auth.json`. Make any account the official default sign-in.
- **Keeping sign-ins valid**: local checks, online authentication checks and automatic renewal per account. For the current Claude account, renewal follows the official lock protocol together with the CLI. If a renewal response is lost, AuthBaton reconciles automatically and never resends old credentials.
- **Quota**: every 5 minutes, a read-only query of the 5-hour and weekly windows, model windows, remaining resets and their expiry. Queries come faster when an account is close to empty.
- **Switching and resets**: each account has a low line and an order (reset first, switch first, reset only, switch only). When an account reaches its line, AuthBaton acts automatically and keeps a record. Off by default.
- **Codex desktop (experimental)**: connects to the official desktop app through a local debugging port and switches accounts without quitting it. Running sessions can be paused and continued after the switch.
- **Claude**: switches accounts without quitting the client; running sessions pick up the change within about 30 seconds. Optional 5-hour window warm-up: each day at a set hour, one Haiku message is sent through the official CLI. Off by default.
- **Claude desktop**: keeps the local Code session list of every account and organization in the Claude desktop app the same, so switching accounts no longer leaves the sidebar empty (transcripts were always shared in `~/.claude/projects`). Switches the desktop app to another account without signing out (macOS): Claude quits, its sign-in is set aside and the other one put back, and Claude opens again. Snapshots and a 30-day trash make every change undoable. Ported from [cc-same](https://github.com/songkeys/cc-same) (MIT). Background sync is off by default.
- **Integrations**: the pi extension takes access tokens per account and gives Codex models Codex's own web search (`web_run`). The Claude Code plugin reports quota, and a turn cut off by the quota limit resumes automatically after the switch.
- **Personal cloud (optional)**: a Worker deployed to your own Cloudflare account renews tokens centrally, so several devices share the same accounts. See [`cloud/`](cloud/README.md).

> [!TIP]
> Claude accounts in the same Team organization share the prompt cache. After a switch, the next request still reads the conversation's cache, so the new account does not write the whole context to the cache again. In our test (Claude Code 2.1.303, Haiku 5.5 and Opus 5.5, five switches back and forth in one conversation), the first request after every switch read the entire earlier prefix from the cache (99.9%). Accounts in different organizations were not tested.

## Security boundaries

- Credentials exist only in AuthBaton's Keychain Vault, the storage the official clients need, and private pipes between processes. They are never written to SQLite, the UI's IPC or logs.
- There is one exception for command-line arguments. When an official Claude or Codex Keychain entry is larger than the 4096-byte single-line limit of `security -i`, it is passed to `/usr/bin/security` as a command-line argument instead. While that write runs, processes of the same macOS user can see the entry in the process list.
- With Codex desktop hot switching on, AuthBaton sets `CODEX_ELECTRON_DISABLE_QUIT_CONFIRMATION=1` in the login session environment (`launchctl setenv`) while it runs. It clears the variable when hot switching is turned off or AuthBaton quits. This keeps a desktop app opened from the Dock, or restarted after an update, from stopping at the quit confirmation when AuthBaton restarts it in debugging mode. A desktop app that AuthBaton opens itself gets `=0` and asks for confirmation on quit as usual. Before a restart, AuthBaton looks in the desktop app's own log (`~/Library/Logs/com.openai.codex/`) for the line that says the process has finished starting, and does not let it quit before then. It reads only that line and keeps nothing from the log.
- Claude desktop: switching moves the desktop app's whole sign-in (its `Cookies`, `Local Storage`, `Session Storage`, `IndexedDB` and `WebStorage`, and three keys of its `config.json`) into `~/Library/Application Support/AuthManager/claude-desktop/logins/<account>` and back, renamed on the same disk while Claude is closed. Claude encrypts these with its own Keychain key; AuthBaton never decrypts, reads, renews or sends them. Session lists are only copied, never linked (Claude stops saving through a symlinked folder). AuthBaton starts Claude with an empty environment.
- The UI process never gets credentials. The Rust service is the only source of account state, and IPC responses use a field allowlist.
- Apart from the warm-up you turn on for a Claude account, AuthBaton sends no model requests and never uses one as an authentication check.
- The runtime directory `~/Library/Application Support/AuthManager/` has mode 0700; sockets and the database are 0600. The desktop operation log `desktop-log.jsonl` (0600, rotated once it passes 512 KB) records only the time, the operation on the Codex desktop app, the account ID and a fixed reason. It contains no credentials. These measures protect against mistakes and credential leaks; they cannot defend against malicious programs running as the same macOS user.

## Anthropic's terms

Claude Code's [Legal and compliance](https://code.claude.com/docs/en/legal-and-compliance#authentication-and-credential-use) page sets rules for signing in with a Claude subscription. The table maps each rule to what AuthBaton does, checked against the page on 2026-11-02. Where AuthBaton departs from a rule, the table says so.

| Anthropic's rule | What AuthBaton does |
|---|---|
| Sign-in to a Claude account must complete through Anthropic's own flow. Third parties may not offer Claude.ai login in their own apps. | Follows it. Adding or re-signing an account runs the official `claude auth login --claudeai` in an isolated directory, and you sign in on Anthropic's pages. AuthBaton only shows the link, so you can open it or copy it into the browser you want. It has no sign-in of its own. |
| Developers may not collect, store or intermediate Claude.ai credentials or session tokens. | **Departs.** Accounts other than the current one keep their tokens in AuthBaton's Keychain Vault, and AuthBaton renews them itself with Claude Code's OAuth client. With personal cloud on, the Worker in your own Cloudflare account holds and renews them. For the Claude desktop app, the sign-ins set aside for switching stay on disk as the encrypted files Claude wrote; AuthBaton never opens them. The tokens stay on your Mac or in your own Cloudflare account. |
| Subscription credentials may not be used to route requests on behalf of users. | Follows it. AuthBaton has no server, sends no model requests for anyone else and does not share accounts. Conversations go from the unmodified Claude Code to Anthropic. The only model request AuthBaton starts is the optional warm-up, which runs through the official CLI. The pi extension gets Codex tokens only, never Claude ones. |
| OAuth is meant for subscribers' ordinary use of Claude Code and Anthropic's native apps. Signing in to the unmodified Claude Code with your own subscription is allowed. | **Partly.** Claude Code is not modified. AuthBaton itself also calls Anthropic's account endpoints with the tokens: renewal, quota and profile queries, and the reset cards you choose to use. None of these is a model request. |
| Advertised usage limits for Pro, Max and Ultra plans assume ordinary, individual usage. | **Your call.** Every account is meant to be your own. Automatic switching when an account runs low is off by default; turning it on moves your work to another of your subscriptions. |

Switching writes the chosen account into Claude Code's own storage while holding Claude Code's locks. Anthropic's documentation covers using several accounts through a separate `CLAUDE_CONFIG_DIR` for each, not switching the default sign-in. Anthropic may enforce these rules without notice, and AuthBaton cannot promise that an account will not be restricted.

## Architecture

```mermaid
flowchart TD
    UI[SwiftUI window and menu bar] -->|Unix socket JSON| S[Rust account service]
    S -->|private pipe| H["credential-store: Vault / SQLite"]
    S -->|private pipe| K["/usr/bin/security: official Keychain items"]
    S --> L[Official Codex / Claude CLI sign-in]
    S -->|curl| API[Official renewal and quota APIs]
    S -->|"private pipe: access token"| B[desktop-bridge]
    B -->|local CDP| D[Codex desktop]
    P[pi extension] -->|pi.sock| S
    M[Claude Code plugin] -->|claude-mod.sock| S
    S -.->|optional| W[Personal cloud Worker]
```

| Path | Contents |
|---|---|
| `services/auth-service/` | Rust account service: account directory, renewal, quota, switching, automatic strategy, IPC |
| `macos/Sources/AuthManager/` | SwiftUI interface and menu bar |
| `macos/Sources/CredentialStore/` | Fixed-version Keychain Vault / SQLite component |
| `macos/Sources/DesktopBridge/` | Codex desktop connection helper and the scripts it injects into the page |
| `integrations/pi/` | pi extension and install script |
| `integrations/claude-code/` | Claude Code companion plugin |
| `cloud/` | Personal cloud Worker |
| `scripts/` | Build script, UI preview and interaction checks, synthetic tests |

## Install

Download the DMG from [Releases](https://github.com/WUXM5/AuthBaton/releases/latest) and drag `AuthManager.app` to Applications. It needs macOS 14 or later on Apple silicon.

The app is ad-hoc signed and not notarized by Apple, so macOS blocks it when it comes from the internet. Before you open it the first time, remove the download quarantine:

```bash
xattr -dr com.apple.quarantine /Applications/AuthManager.app
```

## Build

Requirements:

- To run: macOS 14 or later.
- To build: Rust 1.89+ and the **macOS 26.5 SDK** from the Xcode Command Line Tools. `scripts/build.sh` pins this SDK because the 27.0 SDK declares the SwiftUI macros without shipping the macro plugin.
- To test: Node 22.6+.

```bash
cargo fetch --locked          # the build script builds offline, so fetch the dependencies first
bash scripts/build.sh         # writes dist/AuthManager.app (ad-hoc signed locally)
open dist/AuthManager.app
```

Set `AUTHMANAGER_BUILD_APP=<path>.app` to build somewhere else, so you do not overwrite the running app.

> [!WARNING]
> On first launch, the bundled `credential-store` is copied to `~/Library/Application Support/AuthManager/credential-store-v1`. The Keychain items in the Vault trust only this file's code signature. Do not delete or replace it, or the saved credentials can no longer be read.

## Tests

```bash
cargo test --locked --offline -- --test-threads=1      # must run serially: some tests share global state
cargo clippy --locked --offline --all-targets -- -D warnings
for t in scripts/test-desktop-*.ts scripts/test-automatic-*.ts; do node --experimental-strip-types "$t"; done
node --test cloud/test/*.test.ts
(cd integrations/pi && node --experimental-strip-types --test test-*.ts)   # needs pi installed globally
bash scripts/accounts-preview/render.sh interact        # builds the production UI against a synthetic service and drives interaction checks with OCR
```

Tests that need a real environment are skipped by default:

- `cargo test --locked --offline native_ -- --ignored --test-threads=1` needs the app built first, and reads and writes the system Keychain (synthetic accounts only).
- `scripts/claude-protocol/live-hot-switch.py` drives the real Claude CLI with an isolated `CLAUDE_CONFIG_DIR`.

## Integrations

- **pi**: `node --experimental-strip-types integrations/pi/install.ts` previews the changes; add `--install` to install the extension. Then run `/authmanager` in pi to refresh the account list, and choose the AuthManager Codex provider in `/model`.
- **Claude Code**: `bash integrations/claude-code/install.sh` copies the plugin into the app's data directory and prints that directory. In `~/.claude/settings.json`, set `CLAUDE_CODE_PLUGIN_DIRS` under `env` to that directory.
- **Personal cloud**: see [`cloud/README.md`](cloud/README.md).

## License

AuthBaton is released under the [MIT License](LICENSE.md).

The Claude desktop session sync and account switching in `services/claude-desktop/` are derived from [cc-same](https://github.com/songkeys/cc-same) 0.1.12 by Songkeys, released under the MIT License; its notice is in [`services/claude-desktop/LICENSE-cc-same.txt`](services/claude-desktop/LICENSE-cc-same.txt).

The description and command schema of the `web_run` tool in `integrations/pi/authmanager-codex/web-run.ts` are adapted from [OpenAI Codex](https://github.com/openai/codex) rust-v0.163.0, Copyright 2025 OpenAI, licensed under the [Apache License 2.0](http://www.apache.org/licenses/LICENSE-2.0); the file's header says what changed.
