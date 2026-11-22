"""Validate actual pixels, refresh and ambiguous titles on a private Labwc seat."""
import json
from pathlib import Path
import select
import subprocess
import tomllib


def run(args, env, root, children, logs, ctl, wait, windows):
    report = Path(args.report).parent
    marker = root / "green-frame"

    def fixture(color, suffix):
        log = (root / f"{color}-{suffix}.log").open("w+")
        logs.append(log)
        command = [args.probe, f"--preview-{color}"]
        if color == "red" and suffix == "original":
            command.append(marker)
        process = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        children.append(process)
        return process

    def group():
        return [row for row in windows() if row[2] == '"mosaic-stage-0-probe"']

    originals = [fixture("red", "original"), fixture("blue", "original")]
    wait(lambda: len(group()) == 2)
    ctl("config", "set", "dock.magnification=false\ndock.hover.cards_show_delay_ms=100")
    log = (root / "live-pointer.log").open("w+")
    logs.append(log)
    pointer = subprocess.Popen(
        [args.pointer, "1280", "720"], env=env, stdin=subprocess.PIPE,
        stdout=subprocess.PIPE, stderr=log, text=True, bufsize=1,
    )
    children.append(pointer)

    def send(command):
        pointer.stdin.write(command + "\n")
        pointer.stdin.flush()
        assert select.select([pointer.stdout], [], [], 3)[0]
        assert pointer.stdout.readline().strip() == "ok"

    def hover():
        dock = tomllib.loads(ctl("dock", "state").stdout)
        assert dock["width"] > 0 and dock["height"] > 0
        send("move 12 400")
        send(f"move {(1280 - dock['width']) // 2 + 52} {720 - 12 - dock['height'] + 55}")

    def count(colors):
        ppm = subprocess.check_output([args.grim, "-t", "ppm", "-o", "HEADLESS-1", "-"], env=env, timeout=5)
        magic, dimensions, max_value, pixels = ppm.split(b"\n", 3)
        assert (magic, dimensions, max_value) == (b"P6", b"1280 720", b"255")
        # Sample entirely inside the popover's column at the middle of its cards.
        # The source windows can be behind the popover and visible beside it.
        region = b"".join(pixels[(y * 1280 + 504) * 3:(y * 1280 + 652) * 3]
                          for y in (*range(405, 457), *range(530, 582)))
        return all(region.count(color) > 3500 for color in colors)

    red, blue, green = bytes((251, 25, 79)), bytes((20, 170, 246)), bytes((34, 217, 82))
    hover()
    wait(lambda: len(ctl("dock", "cards").stdout.split()) == 2)
    wait(lambda: count((red, blue)), seconds=5)
    subprocess.run([args.grim, "-o", "HEADLESS-1", report / "preview-live-cards.png"], env=env, check=True, timeout=5)
    marker.touch()
    wait(lambda: count((green, blue)) and not count((red,)), seconds=5)

    send("move 12 400")
    wait(lambda: not ctl("dock", "cards").stdout.split())
    for row in group():
        ctl("window", "minimize", *row[:2])
    wait(lambda: all(row[4] == "true" for row in group()))
    hover()
    wait(lambda: len(ctl("dock", "cards").stdout.split()) == 2)
    wait(lambda: count((green, blue)), seconds=5)

    duplicate = fixture("red", "duplicate")
    wait(lambda: len(group()) == 3)
    hover()
    wait(lambda: len(ctl("dock", "cards").stdout.split()) == 3)
    wait(lambda: not count((green,)), seconds=4)
    duplicate.terminate()
    duplicate.wait(timeout=4)
    wait(lambda: len(group()) == 2)
    hover()
    wait(lambda: len(ctl("dock", "cards").stdout.split()) == 2)
    wait(lambda: count((green, blue)), seconds=5)
    send("move 12 400")
    wait(lambda: not ctl("dock", "cards").stdout.split())
    for process in originals:
        process.terminate()
        process.wait(timeout=4)
    wait(lambda: not group())
    result = {"passed": True, "two_distinct_window_frames": True,
              "pixels_refresh_while_hovered": True, "minimized_windows_capture_after_reopen": True,
              "matching_recovers_after_duplicate_closes": True}
    (report / "live-cards.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
