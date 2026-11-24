"""Exercise overflow in a narrow private Labwc session."""
import json
from pathlib import Path
import select
import subprocess
import tomllib


def run(args, env, processes, logs, ctl, wait):
    directory = Path(args.report).parent
    log = (directory / "overflow-fixture.log").open("w+")
    logs.append(log)
    fixture = subprocess.Popen([args.tray_fixture, "--many-items"], env=env, stdout=log, stderr=log)
    processes.append(fixture)

    def admin(method, *params, ok=True):
        cmd = ["busctl", "--user", "--timeout=3s", "call", "org.mosaic.TrayFixture",
               "/Fixture", "org.mosaic.TrayFixture", method, *params]
        result = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=5)
        if ok:
            result.check_returncode()
        return result if not ok else result.stdout

    def state():
        return tomllib.loads(ctl("tray", "state").stdout)

    def panel_state():
        return tomllib.loads(ctl("outputs").stdout)["outputs"][0]

    def inline_items():
        return [b for b in state()["buttons"] if b["mapped"] and not b["overflow"]]

    def check_compact_panel():
        panel = panel_state()
        assert panel["panel_width"] == 640 and panel["panel_height"] == 32
        items = inline_items()
        assert 0 < len(items) < 16
        assert all(30 <= item["width"] <= 34 and item["height"] >= 28 for item in items), items
        assert all(0 <= item["x"] and item["x"] + item["width"] <= 640 for item in items)
        more = overflow()
        trailing_controls = 640 - (more["x"] + more["width"])
        assert 0 < trailing_controls <= 164, trailing_controls
        return trailing_controls

    wait(lambda: admin("Events", ok=False).returncode == 0 and state()["available"])
    admin("DBusMenu", "b", "true")
    for index in range(16):
        admin("Register", "s", f"/Tray{index:02d}")

    def overflow():
        return next((item for item in state()["overflow"] if item["hidden_count"]), None)

    more = wait(overflow)
    assert 0 < more["hidden_count"] < 16
    check_compact_panel()

    plog = (directory / "overflow-pointer.log").open("w+")
    logs.append(plog)
    pointer = subprocess.Popen([args.pointer, "640", "480"], env=env,
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=plog, text=True, bufsize=1)
    processes.append(pointer)

    def send(command):
        pointer.stdin.write(command + "\n")
        pointer.stdin.flush()
        assert select.select([pointer.stdout], [], [], 3)[0]
        assert pointer.stdout.readline().strip() == "ok"

    def move_to(rect):
        send(f"move {int(rect['x'] + rect['width'] / 2)} {int(rect['y'] + rect['height'] / 2)}")

    def press(button):
        send(f"button {button} 1")
        send(f"button {button} 0")

    def hidden_button():
        return next((b for b in state()["buttons"]
                     if b["overflow"] and b["mapped"] and b["height"] > 0), None)

    def hover_hidden():
        # GTK maps children before Labwc presents the first popup buffer.
        # Wait for a real pointer enter before delivering the click.
        hidden = hidden_button()
        if hidden and not hidden["hovered"]:
            move_to(hidden)
        return hidden and hidden["hovered"]

    move_to(more)
    press(272)
    wait(lambda: overflow()["open"])
    wait(hover_hidden)
    subprocess.run([args.grim, "-o", "HEADLESS-1", str(directory / "preview-overflow.png")],
                   env=env, check=True, timeout=5)
    press(272)
    wait(lambda: "Activate:" in admin("Events"))
    if not overflow()["open"]:
        move_to(overflow())
        press(272)
        wait(lambda: overflow()["open"])
    wait(hover_hidden)
    press(273)
    wait(lambda: state()["menu"]["open"] and not state()["menu"]["loading"])
    subprocess.run([args.wtype, "-s", "100", "-k", "Escape"], env=env, check=True, timeout=5)
    wait(lambda: not state()["menu"]["open"])
    if overflow()["open"]:
        subprocess.run([args.wtype, "-s", "100", "-k", "Escape"], env=env, check=True, timeout=5)
        wait(lambda: not overflow()["open"])

    def resize(width, height, scale=1):
        subprocess.run([args.randr, "--output", "HEADLESS-1",
                        "--custom-mode", f"{width}x{height}@60Hz", "--scale", str(scale)],
                       env=env, check=True, timeout=5)
        wait(lambda: (panel := panel_state())["panel_width"] == width / scale
             and panel["scale"] == scale)

    resize(1280, 720)
    wait(lambda: not overflow() and len(state()["buttons"]) == 16)

    def mapped_items():
        return {b["key"]: b for b in state().get("buttons", []) if b["mapped"]}

    original = tomllib.loads(ctl("config", "get").stdout)
    initial_items = wait(mapped_items)
    first_key = min(initial_items, key=lambda key: initial_items[key]["x"])
    new_order = '["tray","menu","active","status","spacer","clock","settings"]'
    ctl("config", "set", f"panel.order={new_order}")
    wait(lambda: mapped_items().get(first_key, {}).get("x", 1280) < 32)
    subprocess.run([args.grim, "-o", "HEADLESS-1", str(directory / "preview-panel-layout.png")],
                   env=env, check=True, timeout=5)
    ctl("config", "set", 'panel.hidden=["tray"]')
    wait(lambda: not mapped_items())
    assert ctl("config", "set", 'panel.hidden=["clock"]', ok=False).returncode != 0
    assert tomllib.loads(ctl("config", "get").stdout)["panel"]["hidden"] == ["tray"]
    default_order = json.dumps(original["panel"]["order"])
    ctl("config", "set", f"panel.order={default_order}\npanel.hidden=[]")
    wait(lambda: len(mapped_items()) == 16 and mapped_items()[first_key]["x"] >= initial_items[first_key]["x"] - 4)

    trailing_widths = {}
    for scale in (1, 1.25, 1.5, 2):
        resize(int(640 * scale), int(480 * scale), scale)
        wait(overflow)
        trailing_widths[int(scale * 100)] = check_compact_panel()
    admin("Quit")
    assert fixture.wait(timeout=4) == 0
    wait(lambda: not overflow() and not state().get("buttons", []))
    pointer.stdin.write("quit\n")
    pointer.stdin.flush()
    pointer.wait(timeout=3)
    result = {"passed": True, "overflow_at_640px": True, "panel_height_preserved": True,
              "compact_buttons": True, "trailing_control_widths": trailing_widths,
              "real_hidden_item_activation": True, "hidden_dbusmenu": True, "responds_to_resize": True,
              "owner_loss_clears_overflow": True, "panel_reordered_and_restored": True,
              "tray_hidden_and_restored": True, "mandatory_item_protected": True}
    (directory / "overflow.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
