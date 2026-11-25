"""Exercise quick appearance controls and panel shortcuts in a private Labwc session."""

import json
from pathlib import Path
import re
import select
import subprocess
import time
import tomllib


def run(args, env, processes, ctl, wait, config_file):
    def dialog():
        return tomllib.loads(ctl("dialog", "state").stdout)

    def showing(kind):
        info = dialog()
        return info["mapped"] and info["kind"] == kind and info["connector"] == "HEADLESS-1"

    def settings():
        return tomllib.loads(ctl("config", "get").stdout)

    def keys(*names):
        command = [args.wtype, "-s", "100"]
        for name in names:
            command.extend(["-k", name])
        subprocess.run(command, env=env, check=True, timeout=5)

    ctl("control-center", "HEADLESS-1")
    wait(lambda: showing("control-center"))
    ctl("control-center", "HEADLESS-1")
    wait(lambda: not dialog()["mapped"])

    pointer = subprocess.Popen(
        [args.pointer, "1280", "720"], env=env,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
    )
    processes.append(pointer)

    def send(command):
        pointer.stdin.write(command + "\n")
        pointer.stdin.flush()
        assert select.select([pointer.stdout], [], [], 3)[0]
        assert pointer.stdout.readline().strip() == "ok"

    def click(x, y):
        send(f"move {x} {y}")
        send("button 272 1")
        send("button 272 0")

    send("move 1152 16")
    time.sleep(.15)  # Let GTK bind the new virtual pointer.
    send("button 272 1")
    send("button 272 0")
    wait(lambda: showing("control-center"))
    if args.grim:
        subprocess.run(
            [args.grim, "-o", "HEADLESS-1", str(Path(args.report).with_name("preview-center.png"))],
            env=env, check=True, timeout=5,
        )

    ctl("config", "set", "theme.mode='dark'\ngeneral.reduced_motion=true")
    keys("Return", "End", "Return")
    wait(lambda: settings()["theme"]["mode"] == "system")
    keys("Tab", "Return", "End", "Return")
    wait(lambda: settings()["theme"]["symbolic_icon_weight"] == "bold")
    keys("Tab", "space")
    wait(lambda: not settings()["general"]["reduced_motion"])
    saved = tomllib.loads(config_file.read_text())
    assert saved["theme"]["mode"] == "system"
    assert saved["theme"]["symbolic_icon_weight"] == "bold"
    assert saved["general"]["reduced_motion"] is False

    original = config_file.read_text()
    new_revision = saved["revision"] + 1
    external, count = re.subn(r"(?m)^revision = \d+$", f"revision = {new_revision}", original)
    assert count == 1
    config_file.write_text(external)
    keys("space")
    assert config_file.read_text() == external
    assert settings() == saved
    ctl("reload")
    keys("space")
    wait(lambda: settings()["general"]["reduced_motion"])
    saved = tomllib.loads(config_file.read_text())

    keys("Tab", "Return")
    wait(lambda: showing("settings"))
    keys("Escape")
    wait(lambda: not dialog()["mapped"])

    managed = config_file.with_name("managed.toml")
    config_file.rename(managed)
    config_file.symlink_to(managed)
    ctl("reload")
    try:
        ctl("control-center")
        wait(lambda: showing("control-center"))
        keys("Return")
        wait(lambda: showing("settings"))
        assert tomllib.loads(managed.read_text()) == saved
        keys("Escape")
        wait(lambda: not dialog()["mapped"])
    finally:
        config_file.unlink()
        managed.rename(config_file)
        ctl("reload")

    original_panel = settings()["panel"]

    def set_panel(order, hidden):
        ctl("config", "set", f"panel.order={json.dumps(order)}\npanel.hidden={json.dumps(hidden)}")
        assert settings()["panel"] == {**original_panel, "order": order, "hidden": hidden}

    def open_first(kind, screenshot=None):
        time.sleep(.22)
        click(16, 16)
        try:
            wait(lambda: showing(kind))
        except TimeoutError as error:
            raise AssertionError(f"Expected {kind} from first panel item, got {dialog()}") from error
        if screenshot and args.grim:
            subprocess.run(
                [args.grim, "-o", "HEADLESS-1", str(Path(args.report).with_name(screenshot))],
                env=env, check=True, timeout=5,
            )
        keys("Escape")
        wait(lambda: not dialog()["mapped"])

    shortcuts = {
        "wifi": "wifi",
        "bluetooth": "bluetooth",
        "brightness": "brightness",
        "audio": "audio",
        "power": "power",
        "appearance": "settings",
        "dock": "settings",
    }
    base_order = original_panel["order"]
    base_hidden = original_panel["hidden"]
    open_first("launcher")
    for item, kind in shortcuts.items():
        set_panel([item, *base_order], base_hidden)
        open_first(kind)
    set_panel(["dock", *base_order, *[item for item in shortcuts if item != "dock"]], base_hidden)
    open_first("settings", "preview-panel-shortcuts.png")
    set_panel(["wifi", "audio", *base_order], base_hidden)
    open_first("wifi")
    set_panel(["audio", "wifi", *base_order], base_hidden)
    open_first("audio")
    set_panel(["audio", "wifi", *base_order], ["audio", *base_hidden])
    open_first("wifi")
    set_panel(base_order, base_hidden)
    open_first("launcher")

    pointer.stdin.write("quit\n")
    pointer.stdin.flush()
    assert pointer.wait(timeout=3) == 0

    result = {
        "passed": True, "panel_button_opens": True, "repeat_toggles": True,
        "keyboard_settings_persist": True, "external_config_sync": True,
        "stale_write_rolled_back": True,
        "managed_config_keeps_shortcut": True, "escape_closes": True,
        "individual_panel_shortcuts_open": True, "panel_shortcuts_reorder_hide_and_restore": True,
    }
    (Path(args.report).parent / "control-center.json").write_text(
        json.dumps(result, indent=2) + "\n"
    )
    print(json.dumps(result), flush=True)
