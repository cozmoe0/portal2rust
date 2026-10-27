"""Runs one-line Squirrel snippets in the installed game's script VM and in
ours, and reports where the printed output differs.

    python -I re/tools/vscript_oracle.py re/tools/oracle_cases.txt

The game side uses the `script` console command on a loaded map; our side
runs `cargo run -q -p squirrel --example oracle`. Results are written to
re/out/oracle_game.txt and re/out/oracle_ours.txt.
"""
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import p2game  # noqa: E402


def cases(path):
    with open(path, encoding="utf-8") as f:
        return [l.rstrip("\n") for l in f if l.strip() and not l.startswith("#")]


def squirrel_literal(code):
    """The code as a Squirrel string literal without raw quotes or
    semicolons, which the console would otherwise treat as separators."""
    out = code.replace("\\", "\\\\").replace('"', "\\x22").replace(";", "\\x3b")
    return '"' + out + '"'


def game_outputs(snippets):
    results = []
    with p2game.Game(log_name="oracle_console.log") as g:
        if not g.load_map("sp_a1_intro2"):
            sys.exit("map did not load")
        time.sleep(5)
        for i, code in enumerate(snippets):
            g.con.take()
            wrapped = f'print("<<{i}<"); {code}; print(">{i}>>")'
            # The console mangles braces, quotes and semicolons, so the code
            # is rebuilt from character codes in small pieces.
            g.con.send('script ::__p <- ""')
            codes = [ord(c) for c in wrapped]
            for k in range(0, len(codes), 48):
                part = codes[k:k + 48]
                g.con.send(f'script ::__p += format("{"%c" * len(part)}", {", ".join(map(str, part))})')
            g.con.send("script compilestring(::__p)()")
            m = g.con.expect(rf"<<{i}<(.*?)>{i}>>", timeout=15)
            if m:
                results.append(m.group(1))
            else:
                # Errors abort the snippet; capture what the console said.
                results.append("<no output>")
        # The VM's own list of registered functions and classes.
        g.con.take()
        g.con.send("script_help")
        time.sleep(8)
        with open(os.path.join("re", "out", "script_help.txt"), "w", encoding="utf-8") as f:
            f.write(g.con.take())
    return results


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else os.path.join("re", "tools", "oracle_cases.txt")
    snippets = cases(path)
    ours = subprocess.run(
        ["cargo", "run", "-q", "-p", "squirrel", "--example", "oracle", "--", path],
        capture_output=True, text=True,
    ).stdout.split("\n<<<end>>>\n")
    game = game_outputs(snippets)
    os.makedirs(os.path.join("re", "out"), exist_ok=True)
    with open(os.path.join("re", "out", "oracle_game.txt"), "w", encoding="utf-8") as f:
        f.write("\n".join(game))
    with open(os.path.join("re", "out", "oracle_ours.txt"), "w", encoding="utf-8") as f:
        f.write("\n".join(ours))
    same = 0
    for i, code in enumerate(snippets):
        g = game[i] if i < len(game) else "<missing>"
        o = ours[i] if i < len(ours) else "<missing>"
        if g == o:
            same += 1
        else:
            print(f"case {i}: {code}\n  game: {g!r}\n  ours: {o!r}")
    print(f"{same}/{len(snippets)} cases match")


if __name__ == "__main__":
    main()
