"""Verify Labwc's own thumbnail switcher using virtual keys on the test seat."""

import json
from pathlib import Path
import subprocess
import time


def run(args, env, children, ctl, wait, windows, original):
    def own_windows():
        return [window for window in windows() if window[2] == '"mosaic-stage-0-probe"']

    def active():
        return next((tuple(window[:2]) for window in own_windows() if window[3] == "true"), ())

    identifiers = [original]
    for _ in range(2):
        probe = subprocess.Popen([args.probe, "--hold-window"], env=env,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        children.append(probe)
        identifiers.append(wait(lambda: next(
            (tuple(window[:2]) for window in own_windows() if tuple(window[:2]) not in identifiers),
            (),
        )))
    initial = identifiers[-1]
    wait(lambda: active() == initial)

    def send(*keys):
        subprocess.run([args.wtype, *keys], env=env, check=True, timeout=5)

    preview = subprocess.Popen(
        [args.wtype, "-M", "alt", "-k", "Tab", "-s", "1000", "-m", "alt"], env=env,
    )
    children.append(preview)
    time.sleep(.4)
    assert active() == initial, "selection changed before Alt was released"
    if args.grim:
        subprocess.run(
            [args.grim, "-o", "HEADLESS-1", str(Path(args.report).with_name("preview-alt-tab.png"))],
            env=env, check=True, timeout=4,
        )
    assert preview.wait(timeout=5) == 0
    forward = wait(lambda: (selected := active()) and selected != initial and selected)
    assert forward == identifiers[1]

    send("-M", "alt", "-k", "Tab", "-k", "Escape", "-m", "alt")
    assert active() == forward
    send("-M", "alt", "-M", "shift", "-k", "Tab", "-s", "200", "-m", "shift", "-m", "alt")
    reverse = wait(lambda: (selected := active()) and selected != forward and selected)
    assert reverse != initial

    for epoch, ident in identifiers[1:]:
        ctl("window", "close", epoch, ident)
    wait(lambda: len(own_windows()) == 1)
    result = {"passed": True, "alt_release_activates": True,
              "escape_cancels": True, "shift_alt_tab_reverses": True}
    Path(args.report).with_name("alt-tab.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
