"""Capture reference screenshots from the installed game for parity checks.

Reads a shot list (one view per line: name map x y z pitch yaw roll, the
position being the player's feet as with `setpos`), launches the game
windowed at the same size as our renderer with its network console enabled
(random password, so nothing else can use it), then drives it: loads each
map, waits for the player to exist, moves to each view in noclip with the HUD
and viewmodel hidden, and saves a screenshot named after the shot. Files end
up in re/out/ref/<name>.tga.

The player's own config.cfg (and video.txt, if present) are backed up before
the launch and restored afterwards, so the run leaves their settings as they
were. Only cheat cvars, which are not saved to the config, are changed.

    python -I re/tools/capture.py re/shots/chapter1.txt [--width 1600 --height 900]
"""
import argparse
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


def find_install():
    env = os.environ.get("PORTAL2_DIR")
    candidates = [env] if env else []
    candidates.append(r"C:\Program Files (x86)\Steam\steamapps\common\Portal 2")
    for c in candidates:
        if c and os.path.isfile(os.path.join(c, "portal2", "gameinfo.txt")):
            return c
    sys.exit("Portal 2 install not found; set PORTAL2_DIR")


def parse_shots(path):
    shots = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            parts = line.split("#", 1)[0].split()
            if len(parts) < 8:
                continue
            try:
                nums = [float(x) for x in parts[2:8]]
            except ValueError:
                continue
            shots.append((parts[0], parts[1].lower(), nums))
    return shots


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
            m = re.search(pattern, seen)
            if m:
                return m
            time.sleep(0.25)
        return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("shots")
    ap.add_argument("--width", type=int, default=1600)
    ap.add_argument("--height", type=int, default=900)
    ap.add_argument("--map-settle", type=float, default=8.0, help="seconds after a map loads")
    ap.add_argument("--settle", type=float, default=2.5, help="seconds at each view before the screenshot")
    ap.add_argument("--out", default=os.path.join("re", "out", "ref"))
    args = ap.parse_args()

    shots = parse_shots(args.shots)
    if not shots:
        sys.exit("no shots")
    if game_running():
        sys.exit("Portal 2 is already running; close it first")
    root = find_install()
    game = os.path.join(root, "portal2")
    cfg_dir = os.path.join(game, "cfg")
    shot_dir = os.path.join(game, "screenshots")
    backup_dir = os.path.join("re", "out", "capture_backup")
    os.makedirs(backup_dir, exist_ok=True)
    os.makedirs(args.out, exist_ok=True)

    protected = ["config.cfg", "video.txt"]
    existed = {}
    for name in protected:
        src = os.path.join(cfg_dir, name)
        existed[name] = os.path.isfile(src)
        if existed[name]:
            shutil.copy2(src, os.path.join(backup_dir, name))
    # Left over from the first version of this tool.
    stale = os.path.join(cfg_dir, "p2r_capture.cfg")
    if os.path.isfile(stale):
        os.remove(stale)

    password = secrets.token_hex(8)
    cmd = [
        os.path.join(root, "portal2.exe"),
        "-novid", "-windowed", "-noborder",
        "-w", str(args.width), "-h", str(args.height),
        "-netconport", str(PORT), "-netconpassword", password,
    ]
    print("launching Portal 2 with its network console on port", PORT)
    subprocess.Popen(cmd, cwd=root)
    con = NetConsole(password, os.path.join("re", "out", "capture_console.log"))
    captured = 0
    try:
        con.connect(timeout=180)
        time.sleep(10)
        # Not saved to the config: hide console notifications in screenshots.
        con.send("con_drawnotify 0")
        current = None
        for name, map_name, (x, y, z, pitch, yaw, roll) in shots:
            if map_name != current:
                print("loading", map_name)
                con.take()
                con.send(f"map {map_name}")
                time.sleep(5)
                if not con.expect(r"setpos -?\d", timeout=300, poll="getpos"):
                    print("  map did not load; skipping its shots")
                    current = map_name
                    continue
                time.sleep(args.map_settle)
                con.send("sv_cheats 1; cl_drawhud 0; r_drawviewmodel 0")
                con.take()
                con.send("noclip")
                m = con.expect(r"noclip (ON|OFF)", timeout=10)
                if m and m.group(1) == "OFF":
                    con.send("noclip")
                current = map_name
            con.send(f"setpos {x:.3f} {y:.3f} {z:.3f}; setang {pitch:.3f} {yaw:.3f} {roll:.3f}")
            time.sleep(args.settle)
            con.send(f"cl_screenshotname p2r_{name}; screenshot")
            # The game writes <name>.tga and then tries to rename it to
            # <name>_NNNN.tga (which fails on this install); accept either.
            final = re.compile(re.escape(f"p2r_{name}") + r"(_\d+)?\.tga$", re.I)
            end = time.time() + 30
            found = None
            while time.time() < end and not found:
                time.sleep(0.5)
                names = os.listdir(shot_dir) if os.path.isdir(shot_dir) else []
                found = next((f for f in names if final.match(f)), None)
            if not found:
                print("  no screenshot for", name)
                continue
            path = os.path.join(shot_dir, found)
            size = -1
            while size != os.path.getsize(path):
                size = os.path.getsize(path)
                time.sleep(0.5)
            shutil.move(path, os.path.join(args.out, name + ".tga"))
            captured += 1
            print("  captured", name)
        con.send("cl_screenshotname \"\"; con_drawnotify 1")
        con.send("quit")
        end = time.time() + 60
        while game_running() and time.time() < end:
            time.sleep(2)
    finally:
        if game_running():
            subprocess.run(["taskkill", "/IM", "portal2.exe"], capture_output=True)
            time.sleep(5)
        for name in protected:
            dst = os.path.join(cfg_dir, name)
            if existed[name]:
                shutil.copy2(os.path.join(backup_dir, name), dst)
            elif os.path.isfile(dst):
                os.remove(dst)
    print(f"{captured}/{len(shots)} shots captured into {args.out}")


if __name__ == "__main__":
    main()
