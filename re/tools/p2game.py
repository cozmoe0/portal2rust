"""Shared helpers for driving the installed game from analysis tools.

Launches the game with its network console on a local port and a random
password, and talks to it line by line. Used by capture.py and
vscript_oracle.py.
"""
import os
import re
import secrets
import shutil
import socket
import subprocess
import sys
import threading
import time

PORT = 2121
PROTECTED = ["config.cfg", "video.txt"]


def find_install():
    env = os.environ.get("PORTAL2_DIR")
    candidates = [env] if env else []
    candidates.append(r"C:\Program Files (x86)\Steam\steamapps\common\Portal 2")
    for c in candidates:
        if c and os.path.isfile(os.path.join(c, "portal2", "gameinfo.txt")):
            return c
    sys.exit("Portal 2 install not found; set PORTAL2_DIR")


def game_running():
    out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq portal2.exe"], capture_output=True, text=True).stdout
    return "portal2.exe" in out.lower()


class NetConsole:
    """Line-based connection to the game's network console."""

    def __init__(self, password, log_path):
        self.password = password
        self.buf = ""
        self.lock = threading.Lock()
        self.sock = None
        self.log = open(log_path, "w", encoding="utf-8", errors="replace")

    def connect(self, timeout):
        end = time.time() + timeout
        while time.time() < end:
            try:
                self.sock = socket.create_connection(("127.0.0.1", PORT), timeout=5)
                break
            except OSError:
                time.sleep(2)
        if not self.sock:
            raise RuntimeError("could not reach the game's network console")
        self.sock.settimeout(None)
        threading.Thread(target=self._reader, daemon=True).start()
        self.send(f"PASS {self.password}")

    def _reader(self):
        while True:
            try:
                data = self.sock.recv(65536)
            except OSError:
                return
            if not data:
                return
            text = data.decode("utf-8", errors="replace")
            self.log.write(text)
            self.log.flush()
            with self.lock:
                self.buf += text

    def send(self, line):
        self.sock.sendall((line + "\n").encode())

    def take(self):
        with self.lock:
            out, self.buf = self.buf, ""
        return out

    def expect(self, pattern, timeout, poll=None, every=2.0):
        """Waits for `pattern` in the console output, re-sending `poll`."""
        end = time.time() + timeout
        seen = ""
        last = 0.0
        while time.time() < end:
            if poll and time.time() - last > every:
                self.send(poll)
                last = time.time()
            seen += self.take()
            m = re.search(pattern, seen, re.S)
            if m:
                return m
            time.sleep(0.25)
        return None


class Game:
    """Context manager: launches the game, backs up and restores settings."""

    def __init__(self, width=1600, height=900, log_name="game_console.log", extra_args=()):
        self.width, self.height = width, height
        self.extra_args = list(extra_args)
        self.root = find_install()
        self.cfg_dir = os.path.join(self.root, "portal2", "cfg")
        self.backup_dir = os.path.join("re", "out", "capture_backup")
        self.log_path = os.path.join("re", "out", log_name)
        self.con = None

    def __enter__(self):
        if game_running():
            sys.exit("Portal 2 is already running; close it first")
        os.makedirs(self.backup_dir, exist_ok=True)
        self.existed = {}
        for name in PROTECTED:
            src = os.path.join(self.cfg_dir, name)
            self.existed[name] = os.path.isfile(src)
            if self.existed[name]:
                shutil.copy2(src, os.path.join(self.backup_dir, name))
        password = secrets.token_hex(8)
        cmd = [
            os.path.join(self.root, "portal2.exe"),
            "-novid", "-windowed", "-noborder",
            "-w", str(self.width), "-h", str(self.height),
            "-netconport", str(PORT), "-netconpassword", password,
        ] + self.extra_args
        print("launching Portal 2 with its network console on port", PORT)
        subprocess.Popen(cmd, cwd=self.root)
        self.con = NetConsole(password, self.log_path)
        self.con.connect(timeout=180)
        time.sleep(10)
        return self

    def load_map(self, name, timeout=300):
        self.con.take()
        self.con.send(f"map {name}")
        time.sleep(5)
        return self.con.expect(r"setpos -?\d", timeout=timeout, poll="getpos") is not None

    def __exit__(self, *exc):
        try:
            if self.con and game_running():
                self.con.send("quit")
                end = time.time() + 60
                while game_running() and time.time() < end:
                    time.sleep(2)
        finally:
            if game_running():
                subprocess.run(["taskkill", "/IM", "portal2.exe"], capture_output=True)
                time.sleep(5)
            for name in PROTECTED:
                dst = os.path.join(self.cfg_dir, name)
                if self.existed.get(name):
                    shutil.copy2(os.path.join(self.backup_dir, name), dst)
                elif os.path.isfile(dst):
                    os.remove(dst)
        return False
