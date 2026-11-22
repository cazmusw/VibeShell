"""Check the temperature page and opt-in panel indicator in an isolated seat."""
import json
from pathlib import Path
import select
import subprocess
import time
import tomllib


def run(args, env, children, ctl, wait):
    snapshot = tomllib.loads(ctl("temperatures", "status").stdout)
    assert isinstance(snapshot["sensors"], list)
    assert all(sensor["state"] in ("value", "unavailable", "suspended") for sensor in snapshot["sensors"])
    original = tomllib.loads(ctl("config", "get").stdout)["panel"]
    assert "temperatures" not in original["order"]

    def dialog(kind):
        state = tomllib.loads(ctl("dialog", "state").stdout)
        return state["mapped"] and state["kind"] == kind

    def screenshot(name):
        if args.grim:
            subprocess.run([args.grim, "-o", "HEADLESS-1", Path(args.report).with_name(name)],
                           env=env, check=True, timeout=5)

    ctl("temperatures", "show", "HEADLESS-1")
    wait(lambda: dialog("temperatures"))
    wait(lambda: tomllib.loads(ctl("dialog", "state").stdout)["connector"] == "HEADLESS-1")
    time.sleep(.25)
    screenshot("preview-temperatures.png")
    ctl("dialog", "close")
    wait(lambda: not dialog("temperatures"))

    order = ["temperatures", *original["order"]]
    ctl("config", "set", f"panel.order={json.dumps(order)}")
    wait(lambda: tomllib.loads(ctl("config", "get").stdout)["panel"]["order"] == order)
    pointer = subprocess.Popen([args.pointer, "1280", "720"], env=env, stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, text=True, bufsize=1)
    children.append(pointer)

    def send(command):
        pointer.stdin.write(command + "\n")
        pointer.stdin.flush()
        assert select.select([pointer.stdout], [], [], 3)[0]
        assert pointer.stdout.readline().strip() == "ok"

    send("move 16 16")
    time.sleep(.18)
    send("button 272 1")
    send("button 272 0")
    wait(lambda: dialog("temperatures"))
    screenshot("preview-temperature-panel.png")
    ctl("dialog", "close")
    ctl("config", "set", f"panel.order={json.dumps(original['order'])}")
    pointer.stdin.write("quit\n")
    pointer.stdin.flush()
    assert pointer.wait(timeout=3) == 0
    result = {"passed": True, "status_toml": True, "output_selected": True,
              "opt_in_temperature_panel_opens_page": True}
    Path(args.report).with_name("temperatures.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
