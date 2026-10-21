#!/usr/bin/env python3
"""PTY integration tests for the Bash frontends.

The Bash frontends are only meaningful inside a real terminal: they bind
keys, repaint the line and hook into the prompt. A `bash -n` check proves
nothing about them, so these tests drive an interactive Bash through a
pseudo-terminal and look at what actually reaches the screen.

The regression this file exists for is the one that made every new terminal
greet the user with

    [ble: press RET to continue]

which is ble.sh's internal placeholder prompt. It appears when ble.sh's own
prompt-rendering chain is aborted half-way through -- most often because a
key binding was made through ble.sh's low-level API with a key *name* where
a key *code* was expected, and the arithmetic error that followed aborted
the enclosing command.

The tests are skipped (not failed) when ble.sh is not installed, so the
suite still runs on a machine that only has Readline.
"""

import argparse
import fcntl
import os
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PSEUDO_PROMPT = "ADAPTIVE-TEST> "


# ---------------------------------------------------------------------------
# pty plumbing
# ---------------------------------------------------------------------------


class Shell:
    """An interactive Bash running on a pseudo-terminal."""

    def __init__(self, home, env=None, argv=("bash", "-i"), cols=120, rows=40):
        self.home = home
        environment = dict(os.environ)
        environment["HOME"] = home
        environment["TERM"] = "xterm-256color"
        environment["PS1"] = PSEUDO_PROMPT
        environment["PS2"] = "> "
        environment.pop("BASH_ENV", None)
        if env:
            environment.update(env)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            try:
                os.execvpe(argv[0], list(argv), environment)
            except Exception:  # pragma: no cover - only on exec failure
                os._exit(127)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.buffer = bytearray()

    def read(self, seconds=1.0):
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([self.fd], [], [], 0.05)
            if not ready:
                continue
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                break
            if not data:
                break
            self.buffer.extend(data)
        return bytes(self.buffer)

    def take(self):
        out = bytes(self.buffer)
        self.buffer.clear()
        return out

    def send(self, data, settle=1.0):
        os.write(self.fd, data)
        time.sleep(0.05)
        return self.read(settle)

    def close(self):
        try:
            os.write(self.fd, b"\x03exit\n")
        except OSError:
            pass
        time.sleep(0.2)
        try:
            os.close(self.fd)
        except OSError:
            pass
        try:
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)
        except Exception:
            pass


def find_ble_sh():
    for candidate in (
        os.environ.get("BLE_SH", ""),
        os.path.join(os.environ.get("HOME", ""), ".local/share/blesh/ble.sh"),
        os.path.join(os.environ.get("HOME", ""), ".blesh/ble.sh"),
    ):
        if candidate and os.path.isfile(candidate):
            return candidate
    return None


# ---------------------------------------------------------------------------
# fixtures
# ---------------------------------------------------------------------------


def build_home(root, binary, blesh, readline_only=False):
    """An isolated HOME with the adaptive binary and a Bash rc file."""
    home = os.path.join(root, "home")
    os.makedirs(os.path.join(home, ".local/bin"), exist_ok=True)
    os.makedirs(os.path.join(home, ".local/share/adaptive"), exist_ok=True)
    os.makedirs(os.path.join(home, ".local/share"), exist_ok=True)

    link = os.path.join(home, ".local/bin/adaptive")
    if not os.path.exists(link):
        os.symlink(binary, link)

    # Generate the frontends the same way the installer does.
    env = dict(os.environ)
    env["HOME"] = home
    for frontend in ("bash", "blesh"):
        subprocess.run(
            [link, "init", frontend],
            env=env,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

    blesh_link = os.path.join(home, ".local/share/blesh")
    if blesh and not readline_only and not os.path.exists(blesh_link):
        os.symlink(os.path.dirname(blesh), blesh_link)

    rc = os.path.join(home, ".bashrc")
    with open(rc, "w") as handle:
        handle.write(
            "case $- in *i*) ;; *) return;; esac\n"
            'export PATH="$HOME/.local/bin:$PATH"\n'
        )
        if blesh and not readline_only:
            handle.write(
                'if [[ -r ${XDG_DATA_HOME:-$HOME/.local/share}/blesh/ble.sh ]]; then\n'
                "  [[ -n ${_ble_version:-} ]] || source ${XDG_DATA_HOME:-$HOME/.local/share}/blesh/ble.sh\n"
                "fi\n"
            )
        handle.write(
            'if [[ -n ${_ble_version:-} ]] || declare -F ble/util/idle.push >/dev/null 2>&1; then\n'
            '  source "$HOME/.local/share/adaptive/init.blesh"\n'
            "else\n"
            '  source "$HOME/.local/share/adaptive/init.bash"\n'
            "fi\n"
        )
        # A hook that dumps the editor state, so the test can look at what
        # the frontends did rather than guessing from the screen.
        handle.write(
            "__adaptive_test_dump() {\n"
            "  {\n"
            '    printf "PROMPT_COMMAND=[%s]\\n" "$PROMPT_COMMAND"\n'
            '    printf "SOURCES=[%s]\\n" "${_ble_complete_auto_source[*]}"\n'
            '    printf "MARK=[%s]\\n" "$_ble_edit_mark_active"\n'
            '    printf "CANDIDATES=[%s]\\n" "${_ADAPTIVE_BLE_CANDIDATES[*]}"\n'
            '    printf "AC_RIGHT=[%s]\\n" "${_ble_decode_auto_complete_kmap_[1114163]}"\n'
            '    printf "AC_LEFT=[%s]\\n" "${_ble_decode_auto_complete_kmap_[1114164]}"\n'
            '    printf "AC_CF=[%s]\\n" "${_ble_decode_auto_complete_kmap_[67108966]}"\n'
            '    printf "AC_CB=[%s]\\n" "${_ble_decode_auto_complete_kmap_[67108962]}"\n'
            '    printf "BINDINGS=[%s]\\n" "$(bind -p 2>/dev/null | grep -c _adaptive_)"\n'
            "  } > \"$ADAPTIVE_TEST_DUMP\" 2>&1\n"
            "}\n"
            "__adaptive_test_bind() {\n"
            "  ble-bind -m emacs -x 'C-x C-t' '__adaptive_test_dump'\n"
            "}\n"
            "if declare -F ble/util/idle.push >/dev/null 2>&1; then\n"
            "  ble/util/idle.push __adaptive_test_bind\n"
            "fi\n"
        )
    return home


# ---------------------------------------------------------------------------
# tests
# ---------------------------------------------------------------------------

FAILURES = []
CHECKS = [0]


def check(condition, description, detail=""):
    CHECKS[0] += 1
    if condition:
        print(f"ok {CHECKS[0]} - {description}")
    else:
        print(f"not ok {CHECKS[0]} - {description}")
        if detail:
            print(f"  # {detail}")
        FAILURES.append(description)
    return bool(condition)


def test_ten_fresh_launches(home, blesh):
    """Ten consecutive fresh shells must not print ble.sh's placeholder."""
    bad = []
    for index in range(10):
        shell = Shell(home)
        shell.read(3.0)
        output = shell.take()
        shell.send(b"echo LAUNCH_OK\n", 1.5)
        output += shell.take()
        shell.close()
        if b"press RET to continue" in output:
            bad.append(index)
        elif b"LAUNCH_OK" not in output:
            bad.append(f"{index} (no output)")
    check(not bad, "ten fresh interactive shells start cleanly", f"failing launches: {bad}")


def test_no_placeholder_after_command(home, blesh):
    """The placeholder must not reappear once a command has run."""
    shell = Shell(home)
    shell.read(3.0)
    shell.take()
    shell.send(b"echo FIRST\n", 1.5)
    shell.take()
    output = shell.send(b"echo SECOND\n", 1.5)
    shell.close()
    check(b"press RET to continue" not in output, "the placeholder never returns after a command")


def test_prompt_is_not_duplicated(home, blesh):
    """Exactly one prompt per command: no doubled or malformed prompts."""
    shell = Shell(home)
    shell.read(3.0)
    shell.take()
    output = shell.send(b"echo PROMPT_COUNT\n", 1.5)
    shell.close()
    count = output.count(PSEUDO_PROMPT.strip().encode())
    check(count <= 2, "the prompt is not duplicated", f"prompt seen {count} times")


def test_inline_suggestion(home, blesh, dump_path):
    """The engine's suggestion is drawn in the dim face, and the arrows cycle."""
    shell = Shell(
        home,
        env={"ADAPTIVE_TEST_DUMP": dump_path},
    )
    shell.read(3.0)
    shell.take()
    typed = shell.send(b"ls", 2.5)
    check(
        b"38;5;245" in typed,
        "the inline suggestion is drawn in the dim face",
        repr(typed[-200:]),
    )
    # The dim text is the suggestion; the plain text is what was typed.
    check(b"ls" in typed, "the typed text is on screen")

    # Ask the shell what it thinks the state is.
    shell.send(b"\x18\x14", 1.5)
    state = ""
    if os.path.exists(dump_path):
        with open(dump_path) as handle:
            state = handle.read()
    check(
        "CANDIDATES=[lshw" in state or "CANDIDATES=[ls" in state,
        "the engine's candidates reach the frontend",
        state,
    )
    check("SOURCES=[adaptive" in state, "Adaptive is registered as an auto-complete source", state)
    check(
        "AC_RIGHT=[1:ble/widget/adaptive-cycle-next]" in state,
        "Right cycles the candidates in the auto-complete keymap",
        state,
    )
    check(
        "AC_LEFT=[1:ble/widget/adaptive-cycle-prev]" in state,
        "Left cycles the candidates in the auto-complete keymap",
        state,
    )
    check(
        "AC_CF=[1:ble/widget/adaptive-cycle-next]" in state,
        "Ctrl-F cycles the candidates in the auto-complete keymap",
        state,
    )
    check(
        "AC_CB=[1:ble/widget/adaptive-cycle-prev]" in state,
        "Ctrl-B cycles the candidates in the auto-complete keymap",
        state,
    )
    check(
        state.count("_adaptive_blesh_history_record") == 1,
        "the history hook is registered exactly once",
        state,
    )
    shell.close()


def test_arrow_cycling_changes_the_suggestion(home, blesh):
    """Pressing Right moves to the next candidate."""
    shell = Shell(home)
    shell.read(3.0)
    shell.take()
    shell.send(b"ls", 2.5)
    first = shell.take()
    after = shell.send(b"\x1b[C", 1.5)
    shell.close()
    dim = lambda blob: blob.split(b"\x1b[0;38;5;245m")[-1].split(b"\x1b")[0]  # noqa: E731
    check(
        dim(first) != dim(after) or b"adaptive-cycle" in after,
        "Right cycles to another candidate",
        f"{dim(first)!r} -> {dim(after)!r}",
    )


def test_readline_backend(home, blesh, dump_path):
    """Without ble.sh the Readline frontend takes over and still works."""
    root = tempfile.mkdtemp(prefix="adaptive-readline-")
    try:
        readline_home = build_home(root, ADAPTIVE_BINARY, blesh, readline_only=True)
        shell = Shell(readline_home, env={"ADAPTIVE_TEST_DUMP": dump_path})
        shell.read(3.0)
        shell.take()
        output = shell.send(b"echo READLINE_OK\n", 1.5)
        shell.close()
        check(b"READLINE_OK" in output, "the Readline backend runs commands")
        check(
            b"press RET to continue" not in output,
            "the Readline backend never shows the ble.sh placeholder",
        )
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_zsh_untouched(home, blesh):
    """The Zsh frontend is untouched by the Bash changes."""
    zsh = shutil.which("zsh")
    if not zsh:
        print("# zsh is not installed; skipping the Zsh check")
        return
    shell = Shell(home, argv=(zsh, "-i"))
    shell.read(3.0)
    shell.take()
    output = shell.send(b"echo ZSH_OK\n", 1.5)
    shell.close()
    check(b"ZSH_OK" in output, "zsh still starts and runs commands")


ADAPTIVE_BINARY = ""


def main():
    global ADAPTIVE_BINARY
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--adaptive", help="path to the adaptive binary")
    parser.add_argument("--blesh", help="path to ble.sh")
    parser.add_argument("-v", "--verbose", action="store_true")
    args = parser.parse_args()

    binary = args.adaptive or os.environ.get("ADAPTIVE_BIN")
    if not binary:
        for candidate in (
            os.path.join(REPO, "target/release/adaptive"),
            os.path.join(REPO, "target/debug/adaptive"),
        ):
            if os.path.isfile(candidate):
                binary = candidate
                break
    if not binary or not os.path.isfile(binary):
        print("Bail out! no adaptive binary; run `cargo build --release` first")
        return 1
    ADAPTIVE_BINARY = os.path.abspath(binary)

    blesh = args.blesh or find_ble_sh()
    if not blesh:
        print("Bail out! ble.sh is not installed; the ble.sh checks are skipped")
        return 0

    root = tempfile.mkdtemp(prefix="adaptive-blesh-pty-")
    try:
        home = build_home(root, ADAPTIVE_BINARY, blesh)
        dump_path = os.path.join(root, "state.txt")
        test_ten_fresh_launches(home, blesh)
        test_no_placeholder_after_command(home, blesh)
        test_prompt_is_not_duplicated(home, blesh)
        test_inline_suggestion(home, blesh, dump_path)
        test_arrow_cycling_changes_the_suggestion(home, blesh)
        test_readline_backend(home, blesh, dump_path)
        test_zsh_untouched(home, blesh)
    finally:
        shutil.rmtree(root, ignore_errors=True)

    print(f"1..{CHECKS[0]}")
    if FAILURES:
        print(f"# {len(FAILURES)} check(s) failed:")
        for failure in FAILURES:
            print(f"#   - {failure}")
        return 1
    print("# all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
