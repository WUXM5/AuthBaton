#!/usr/bin/env python3
"""Live check that a running Claude CLI uses a switched login from its next request on.

Isolated from every real session: the CLI runs with its own CLAUDE_CONFIG_DIR, so its own
Keychain item, config file and locks. The default login and AuthManager's vault are only read
by the Rust steps in claude_live_switch_tests.rs; the copies carry an unusable refresh token.
The account behind each request is told apart by the usage windows Claude Code reports to its
status line, which come from that request's response.

Usage: live-hot-switch.py <new directory under /private/tmp> <vault id of the other Claude account>
"""
import fcntl
import json
import os
import pty
import re
import shutil
import signal
import struct
import subprocess
import sys
import termios
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
CLAUDE = "/opt/homebrew/bin/claude"
ANSI = re.compile(rb"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07|\x1b[()][0-9A-B]|\x1b[=>]")


def step(name: str, root: Path, source: str) -> None:
    env = {**os.environ, "AUTHMANAGER_LIVE_CLAUDE_DIR": str(root / "config"), "AUTHMANAGER_LIVE_SOURCE": source}
    test = f"accounts::claude_live_switch_tests::{name}"
    result = subprocess.run(
        ["cargo", "test", "--locked", "--offline", test, "--", "--ignored", "--exact", "--test-threads=1"],
        cwd=REPO, env=env, capture_output=True, text=True,
    )
    if result.returncode != 0 or "1 passed" not in result.stdout:
        raise SystemExit(f"{name}({source}) failed:\n{result.stdout[-3000:]}\n{result.stderr[-3000:]}")
    print(f"  step {name}({'native' if source == 'native' else 'other'}) ok", flush=True)


class Session:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.screen = bytearray()
        env = {key: os.environ[key] for key in ("HOME", "USER", "LOGNAME", "TMPDIR", "http_proxy", "https_proxy",
                                                  "no_proxy", "NO_PROXY", "NODE_USE_ENV_PROXY") if key in os.environ}
        env.update(PATH="/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin", TERM="xterm-256color", LANG="en_US.UTF-8",
                   SHELL="/bin/zsh", CLAUDE_CONFIG_DIR=str(root / "config"), DISABLE_AUTOUPDATER="1",
                   ENABLE_CLAUDEAI_MCP_SERVERS="false")
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(root / "work")
            os.execve(CLAUDE, [CLAUDE, "--model", "haiku"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 140, 0, 0))
        self.log = open(root / "pty.log", "wb")
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self) -> None:
        while True:
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                return
            if not data:
                return
            self.log.write(data)
            self.log.flush()
            self.screen += data

    def text(self) -> str:
        return ANSI.sub(b"", bytes(self.screen[-20000:])).decode("utf-8", "replace")

    def send(self, line: str) -> None:
        os.write(self.fd, line.encode())
        time.sleep(0.5)
        os.write(self.fd, b"\r")

    def close(self) -> None:
        for _ in range(2):
            os.write(self.fd, b"\x04")
            time.sleep(0.5)
        deadline = time.time() + 15
        while time.time() < deadline:
            if os.waitpid(self.pid, os.WNOHANG)[0]:
                return
            time.sleep(0.2)
        os.kill(self.pid, signal.SIGTERM)  # our own child only
        os.waitpid(self.pid, 0)


def lines(root: Path) -> list[dict]:
    path = root / "statusline.jsonl"
    if not path.exists():
        return []
    return [json.loads(l) for l in path.read_text().splitlines() if l.strip()]


def turn(session: Session, root: Path, prompt: str) -> dict:
    """Send one prompt and return the status-line record after its response settles."""
    before = len(lines(root))
    session.send(prompt)
    deadline = time.time() + 120
    last, settled = before, time.time()
    while time.time() < deadline:
        count = len(lines(root))
        if count != last:
            last, settled = count, time.time()
        record = lines(root)[-1] if count > before else None
        if record and (record.get("context_window") or {}).get("current_usage") and time.time() - settled > 4:
            return record
        time.sleep(0.5)
    raise SystemExit(f"no response to {prompt!r}; screen:\n{session.text()[-3000:]}")


def summary(label: str, record: dict) -> dict:
    limits = record.get("rate_limits") or {}
    usage = (record.get("context_window") or {}).get("current_usage") or {}
    row = {
        "turn": label,
        "five_hour_used": (limits.get("five_hour") or {}).get("used_percentage"),
        "seven_day_used": (limits.get("seven_day") or {}).get("used_percentage"),
        "five_hour_resets_at": (limits.get("five_hour") or {}).get("resets_at"),
        "seven_day_resets_at": (limits.get("seven_day") or {}).get("resets_at"),
        "input_tokens": usage.get("input_tokens"),
        "cache_creation_input_tokens": usage.get("cache_creation_input_tokens"),
        "cache_read_input_tokens": usage.get("cache_read_input_tokens"),
        "model": (record.get("model") or {}).get("id"),
    }
    print(f"  {json.dumps(row, ensure_ascii=False)}", flush=True)
    return row


def main() -> None:
    root, other = Path(sys.argv[1]), sys.argv[2]
    assert str(root).startswith("/private/tmp/") and not root.exists()
    (root / "config").mkdir(parents=True)
    (root / "work").mkdir()
    (root / "config" / "settings.json").write_text(json.dumps({
        "statusLine": {"type": "command", "command": f"{{ cat; echo; }} >> '{root}/statusline.jsonl'; printf live-test"},
    }))
    (root / "config" / ".claude.json").write_text(json.dumps({
        "hasCompletedOnboarding": True, "lastOnboardingVersion": "2.1.284", "numStartups": 5, "theme": "dark",
        "autoUpdates": False,
        "projects": {str(root / "work"): {"hasTrustDialogAccepted": True, "hasCompletedProjectOnboarding": True}},
    }))
    rows: list[dict] = []
    step("live_claude_prepare", root, "native")
    session = Session(root)
    try:
        deadline = time.time() + 60
        while not lines(root):
            if time.time() > deadline:
                raise SystemExit(f"CLI not ready; screen:\n{session.text()[-3000:]}")
            time.sleep(0.5)
        time.sleep(3)
        print("CLI ready (isolated config dir)", flush=True)
        rows.append(summary("1 current login", turn(session, root, "Reply with exactly the single word: one")))
        step("live_claude_switch", root, other)
        switched = time.time()
        step("live_claude_expect", root, other)
        # The CLI keeps a Keychain read for 30 s, so a request right away may still use the old login.
        rows.append(summary(f"2 other, {time.time() - switched:.0f}s after switch",
                            turn(session, root, "Reply with exactly the single word: two")))
        time.sleep(35)
        rows.append(summary(f"3 other, {time.time() - switched:.0f}s after switch",
                            turn(session, root, "Reply with exactly the single word: three")))
        step("live_claude_expect", root, other)
        step("live_claude_switch", root, "native")
        switched = time.time()
        time.sleep(35)
        rows.append(summary(f"4 back, {time.time() - switched:.0f}s after switch",
                            turn(session, root, "Reply with exactly the single word: four")))
        step("live_claude_expect", root, "native")
        config = json.loads((root / "config" / ".claude.json").read_text())
        print(f"  config oauthAccount present: {bool(config.get('oauthAccount'))}", flush=True)
    finally:
        session.close()
        step("live_claude_cleanup", root, "native")
        (root / "screen.txt").write_text(session.text())
        shutil.rmtree(root / "config")
    (root / "result.json").write_text(json.dumps(rows, ensure_ascii=False, indent=1))
    print("done; config dir and Keychain item removed", flush=True)


if __name__ == "__main__":
    main()
