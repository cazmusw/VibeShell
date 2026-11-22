"""Exercise both side docks on an isolated Labwc output and input seat."""
import json
from pathlib import Path
import select
import subprocess
import tomllib


def run(args, env, processes, ctl, wait):
    directory = Path(args.report).parent

    def state():
        return tomllib.loads(ctl("dock", "state").stdout)

    def patch(text):
        return tomllib.loads(ctl("config", "set", text).stdout)

    output = tomllib.loads(ctl("outputs").stdout)["outputs"][0]
    width, height = output["width"], output["height"]
    middle = (height + output["panel_height"]) // 2
    original = state()
    pointer = subprocess.Popen([args.pointer, str(width), str(height)], env=env,
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    processes.append(pointer)

    def send(command):
        pointer.stdin.write(command + "\n")
        pointer.stdin.flush()
        assert select.select([pointer.stdout], [], [], 3)[0]
        assert pointer.stdout.readline().strip() == "ok"

    def image(region):
        return subprocess.check_output([args.grim, "-g", region, "-"], env=env, timeout=5)

    send(f"move {width // 2} 110")
    for edge, strip_x, trigger_x, icon_x in (
        ("left", 24, 1, 65), ("right", width - 36, width - 1, width - 65),
    ):
        region = f"{strip_x},{middle - 25} 12x50"
        before = image(region)
        patch(f"dock.edge='{edge}'")
        dock = wait(lambda: (value if (value := state())["visible"]
                            and value["height"] > value["width"] > 0 else None))
        assert dock["layout_revision"] == original["layout_revision"]
        assert dock["exclusive_zone"] > 0
        wait(lambda: image(region) != before)
        y = middle - dock["height"] // 2 + 55
        send(f"move {icon_x} {y}")
        wait(lambda: state()["max_scale"] > 1.2)
        wait(lambda: bool(ctl("dock", "cards").stdout.strip()))
        subprocess.run([args.grim, "-o", output["connector"], str(directory / f"preview-{edge}.png")],
                       env=env, check=True, timeout=5)
        send(f"move {width // 2} 110")
        wait(lambda: not ctl("dock", "cards").stdout.strip())
        patch("dock.auto_hide=true\ndock.show_delay_ms=100\ndock.hide_delay_ms=250")
        wait(lambda: not state()["visible"] and state()["trigger_mapped"])
        assert state()["trigger_width"] == 2 and state()["trigger_height"] > 60
        send(f"move {trigger_x} {middle}")
        wait(lambda: state()["visible"])
        send(f"move {width // 2} 110")
        wait(lambda: not state()["visible"])
        patch("dock.auto_hide=false")

    pins = tomllib.loads(ctl("config", "get").stdout)["pins"]
    long_list = pins + [f"mosaic-scroll-{n}.desktop" for n in range(30)]
    patch(f"pins={json.dumps(long_list)}")
    dock = wait(lambda: (s if (s := state())["height"] >= height - 64 else None))
    assert dock["height"] <= height - output["panel_height"]
    assert dock["icon_size"] == 32
    x = width - 58

    def pointer_on_resized_dock():
        send(f"move {x} {middle}")
        return state()["max_scale"] > 1.15

    wait(pointer_on_resized_dock)
    for _ in range(65):
        send("axis v 15")
    wait(lambda: not state()["animation_active"])
    # The catalog button is last and starts below the viewport.
    send(f"move {x} {middle + dock['height'] // 2 - 42}")
    send("button 272 1")
    send("button 272 0")
    wait(lambda: tomllib.loads(ctl("dialog", "state").stdout)["kind"] == "launcher")
    ctl("dialog", "close")
    patch(f"pins={json.dumps(pins)}")
    revision = state()["layout_revision"]
    patch("dock.edge='bottom'")
    wait(lambda: (s := state())["visible"] and s["width"] > s["height"] > 0)
    assert state()["layout_revision"] == revision
    assert ctl("config", "set", "dock.edge='top'", ok=False).returncode != 0
    send("move 20 100")
    pointer.stdin.write("quit\n")
    pointer.stdin.flush()
    assert pointer.wait(timeout=3) == 0
    result = {"passed": True, "both_physical_edges": True, "autohide_on_side_edges": True,
              "vertical_magnification_and_cards": True, "layout_revision_stable": True,
              "vertical_overflow_scroll": True, "returns_to_bottom": True, "scale": output["scale"]}
    (directory / "dock-edges.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
