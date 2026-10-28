"""Capture reference screenshots of the installed game's interface.

Launches the game windowed (like capture.py, settings backed up and
restored) and walks a fixed script: the main menu with its static
background (`-nomenuvid`, so frames are comparable), the single player
menu, the new game and load game dialogs (opened with the keyboard, which
is posted to the game window so the real mouse is never touched), then a
map with the pause menu, close captions (`cc_emit`), a chapter title and the
crosshair. Screenshots go to re/out/ref_ui/<name>.tga.

    python -I re/tools/capture_ui.py [--width 1600 --height 900] [--only name,...]
"""
import argparse
import ctypes
import os
import re
import shutil
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from p2game import Game  # noqa: E402

WM_KEYDOWN, WM_KEYUP, WM_CHAR = 0x0100, 0x0101, 0x0102
KEYS = {"down": 0x28, "up": 0x26, "left": 0x25, "right": 0x27, "enter": 0x0D, "escape": 0x1B}
user32 = ctypes.windll.user32


def game_window():
    hwnd = user32.FindWindowW("Valve001", None)
    if hwnd:
        return hwnd
    found = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def cb(hwnd, _):
        buf = ctypes.create_unicode_buffer(256)
        user32.GetWindowTextW(hwnd, buf, 256)
        if buf.value.lower().startswith("portal 2") and user32.IsWindowVisible(hwnd):
            found.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    return found[0] if found else None


def press(hwnd, key, wait=0.6):
    vk = KEYS[key]
    scan = user32.MapVirtualKeyW(vk, 0)
    user32.PostMessageW(hwnd, WM_KEYDOWN, vk, 1 | (scan << 16))
    time.sleep(0.05)
    user32.PostMessageW(hwnd, WM_KEYUP, vk, 1 | (scan << 16) | (0xC0 << 24))
    time.sleep(wait)


class Shooter:
    def __init__(self, game, out, only):
        self.game, self.out, self.only = game, out, only
        self.shot_dir = os.path.join(game.root, "portal2", "screenshots")

    def shot(self, name, settle=1.5):
        if self.only and name not in self.only:
            return
        time.sleep(settle)
        con = self.game.con
        con.send(f"cl_screenshotname p2r_{name}; screenshot")
        final = re.compile(re.escape(f"p2r_{name}") + r"(_\d+)?\.tga$", re.I)
        end = time.time() + 30
        found = None
        while time.time() < end and not found:
            time.sleep(0.5)
            names = os.listdir(self.shot_dir) if os.path.isdir(self.shot_dir) else []
            found = next((f for f in names if final.match(f)), None)
        if not found:
            print("  no screenshot for", name)
            return
        path = os.path.join(self.shot_dir, found)
        size = -1
        while size != os.path.getsize(path):
            size = os.path.getsize(path)
            time.sleep(0.5)
        shutil.move(path, os.path.join(self.out, name + ".tga"))
        print("  captured", name)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--width", type=int, default=1600)
    ap.add_argument("--height", type=int, default=900)
    ap.add_argument("--out", default=os.path.join("re", "out", "ref_ui"))
    ap.add_argument("--only", default="")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    only = set(filter(None, args.only.split(",")))
    with Game(args.width, args.height, log_name="capture_ui_console.log", extra_args=["-nomenuvid"]) as game:
        con = game.con
        con.send("con_drawnotify 0")
        s = Shooter(game, args.out, only)
        time.sleep(8)
        hwnd = game_window()
        if not hwnd:
            sys.exit("game window not found")
        s.shot("mainmenu")
        # Single player menu (focused first), then its dialogs.
        press(hwnd, "enter", 1.5)
        s.shot("singleplayer")
        press(hwnd, "down")
        press(hwnd, "enter", 2.0)
        s.shot("newgame")
        press(hwnd, "escape", 1.5)
        press(hwnd, "down")
        press(hwnd, "enter", 2.0)
        s.shot("loadgame")
        press(hwnd, "escape", 1.5)
        press(hwnd, "escape", 1.5)
        # In a map: HUD pieces, then the pause menu.
        con.take()
        if not game.load_map("sp_a2_laser_intro"):
            sys.exit("map did not load")
        time.sleep(6)
        con.send("sv_cheats 1; closecaption 1")
        s.shot("hud_crosshair", 3.0)
        con.send("cc_emit glados.sp_a2_laser_intro_ending02")
        s.shot("hud_caption", 2.5)
        con.send('ent_fire @chapter_title_text settext "#portal2_Chapter2_Title"; ent_fire @chapter_title_text display')
        s.shot("hud_chapter_title", 4.0)
        con.send("gameui_activate")
        s.shot("pausemenu", 2.0)
        con.send("closecaption 0; con_drawnotify 1; cl_screenshotname \"\"")
    print("done")


if __name__ == "__main__":
    main()
