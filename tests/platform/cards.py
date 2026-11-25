"""Drive Dock clicks and cards against real window state on the private seat."""
import json
from pathlib import Path
import select
import subprocess
import time
import tomllib


def indicator_marks(args, env, output, dock):
    x = output['x'] + (output['width'] - dock['width']) // 2 + 28
    y = output['y'] + output['height'] - 44
    shot = subprocess.check_output(
        [args.grim, '-t', 'ppm', '-s', '1', '-g', f'{x},{y} 48x26', '-'],
        env=env, timeout=5,
    )
    magic, size, maximum, pixels = shot.split(b'\n', 3)
    assert (magic, size, maximum) == (b'P6', b'48 26', b'255')
    ink = bytes.fromhex('bddaff')
    columns = {index // 3 % 48 for index in range(0, len(pixels), 3)
               if pixels[index:index + 3] == ink}
    return sum(column - 1 not in columns for column in columns)


def check_indicators(args, env, ctl, wait, windows):
    output = tomllib.loads(ctl('outputs').stdout)['outputs'][0]

    def dock():
        return tomllib.loads(ctl('dock', 'state').stdout)

    def group():
        return [row for row in windows() if row[2] == '"mosaic-stage-0-probe"']

    def wait_marks(expected):
        try:
            wait(lambda: indicator_marks(args, env, output, dock()) == expected)
        except TimeoutError as error:
            screenshot = Path(args.report).parent / 'window-indicators-failure.png'
            subprocess.run([args.grim, '-o', output['connector'], str(screenshot)],
                           env=env, check=True, timeout=5)
            actual = indicator_marks(args, env, output, dock())
            raise AssertionError(f'Expected {expected} marks, saw {actual}: {dock()}') from error

    baseline = dock()
    wait_marks(0)
    for expected in range(1, 5):
        ctl('launch', 'mosaic-stage-0-probe.desktop')
        wait(lambda: len(group()) == expected)
        wait_marks(expected)
    screenshot = Path(args.report).parent / 'preview-window-indicators.png'
    subprocess.run([args.grim, '-o', output['connector'], str(screenshot)],
                   env=env, check=True, timeout=5)
    for expected in reversed(range(4)):
        row = group()[-1]
        ctl('window', 'close', *row[:2])
        wait(lambda: len(group()) == expected)
        wait_marks(expected)
    assert all(dock()[key] == baseline[key] for key in ('width', 'height', 'layout_revision'))
    result = {'passed': True, 'zero_to_three_dots': True, 'three_dots_and_plus': True,
              'updates_on_close_without_relayout': True}
    (Path(args.report).parent / 'window-indicators.json').write_text(json.dumps(result) + '\n')
    print(json.dumps(result), flush=True)


def run(args, env, processes, logs, ctl, wait, windows):
    def group():
        return sorted(
            (row for row in windows() if row[2] == '"mosaic-stage-0-probe"'),
            key=lambda row: int(row[1]),
        )

    def cards():
        return ctl("dock", "cards").stdout.split()

    ctl("launch", "mosaic-stage-0-probe.desktop")
    first, second = wait(lambda: rows if len(rows := group()) == 2 else None)
    for row in (first, second):
        ctl("window", "minimize", *row[:2])
    wait(lambda: all(row[4] == "true" for row in group()))
    ctl("config", "set",
        "dock.magnification=false\n"
        "dock.hover.cards_show_delay_ms=700\n"
        "dock.hover.cards_hide_delay_ms=450")

    width = int(2560 / args.scale) if args.scale != 1 else 1280
    height = int(1440 / args.scale) if args.scale != 1 else 720
    log = (Path(args.report).parent / "cards-pointer.log").open("w+")
    logs.append(log)
    pointer = subprocess.Popen(
        [args.pointer, str(width), str(height)],
        env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=log, text=True, bufsize=1,
    )
    processes.append(pointer)

    def send(command):
        pointer.stdin.write(command + "\n")
        pointer.stdin.flush()
        assert select.select([pointer.stdout], [], [], 3)[0]
        assert pointer.stdout.readline().strip() == "ok"

    def click():
        send("button 272 1")
        send("button 272 0")

    def hover():
        state = tomllib.loads(ctl("dock", "state").stdout)
        send("move 12 400")
        send(f"move {int((width - state['width']) / 2 + 52)} {height - 12 - state['height'] + 55}")

    hover()
    time.sleep(0.21)
    assert not cards(), "window cards appeared before the configured hover delay"
    wait(lambda: cards() == [first[1], second[1]])
    ctl("config", "set", "dock.hover.cards_enabled=false")
    wait(lambda: not cards())
    hover()
    time.sleep(0.83)
    assert not cards(), "disabled window cards appeared on hover"
    ctl("config", "set", "dock.hover.cards_enabled=true")
    hover()
    wait(lambda: cards() == [first[1], second[1]])
    # Focus and minimized state can change from outside the Dock while hovered.
    ctl("window", "activate", *second[:2])
    wait(lambda: group()[1][3] == "true")
    assert cards() == [first[1], second[1]]
    ctl("window", "minimize", *second[:2])
    wait(lambda: group()[1][4] == "true")
    assert cards() == [first[1], second[1]]
    if args.grim:
        subprocess.run(
            [args.grim, "-o", "HEADLESS-1", str(Path(args.report).parent / "preview-cards.png")],
            env=env, check=True, timeout=5,
        )
    click()
    wait(lambda: group()[0][3:5] == ["true", "false"])
    wait(lambda: not cards())
    click()
    wait(lambda: group()[1][3:5] == ["true", "false"])
    click()
    wait(lambda: group()[1][4] == "true")
    hover()
    wait(lambda: len(cards()) == 2)
    send("move 12 400")
    time.sleep(0.13)
    assert len(cards()) == 2, "window cards closed before the configured leave delay"
    wait(lambda: not cards())

    hover()
    wait(lambda: len(cards()) == 2)
    state = tomllib.loads(ctl("dock", "state").stdout)
    # The final row is immediately above the bottom-anchored Dock.
    send(f"move {int((width - state['width']) / 2 + 52)} {height - 12 - state['height'] - 20}")
    time.sleep(0.30)
    assert cards() == [first[1], second[1]]
    click()
    wait(lambda: group()[1][3:5] == ["true", "false"])
    wait(lambda: not cards())
    ctl("window", "close", *second[:2])
    wait(lambda: len(group()) == 1)
    ctl("window", "activate", *first[:2])
    wait(lambda: group()[0][3] == "true")
    hover()
    wait(lambda: cards() == [first[1]])
    click()
    wait(lambda: group()[0][4] == "true")
    click()
    wait(lambda: group()[0][3:5] == ["true", "false"])

    send("move 12 400")
    pointer.stdin.write("quit\n")
    pointer.stdin.flush()
    pointer.wait(timeout=3)
    result = {"passed": True, "ordered_cards": True, "ordered_activation": True,
              "repeat_minimizes": True, "direct_card_selection": True, "live_state_update": True, "pointer_leave_dismisses_cards": True,
              "configurable_card_delays": True, "cards_setting_can_disable_hover": True,
              "window_images": False}
    (Path(args.report).parent / "cards.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
