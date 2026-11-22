"""Drag an actual URI offer from a separate GTK Wayland client."""
import json
from pathlib import Path
import select
import subprocess
import time
import tomllib


def run(args, env, children, logs, ctl, wait, config_file, installed, marker):
    report = Path(args.report).parent
    width, height = 1280, 720
    pointer = subprocess.Popen([args.pointer, str(width), str(height)], env=env,
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    children.append(pointer)

    def send(command):
        pointer.stdin.write(command + "\n")
        pointer.stdin.flush()
        assert select.select([pointer.stdout], [], [], 3)[0]
        assert pointer.stdout.readline().strip() == "ok"

    def config():
        return tomllib.loads(ctl("config", "get").stdout)

    def drag(label, files, reveal=False):
        path = report / f"{label}.log"
        log = path.open("w+")
        logs.append(log)
        source = subprocess.Popen([args.drop_fixture, *files], env=env, stdout=log, stderr=log)
        children.append(source)
        wait(lambda: "mapped" in path.read_text())
        # The fixture must present a buffer before grabbing the pointer.
        time.sleep(.12)
        send("move 225 160")
        time.sleep(.06)
        send("button 272 1")
        time.sleep(.06)
        send("move 237 161")
        time.sleep(.06)
        send("move 260 163")
        wait(lambda: "\ndrag\n" in path.read_text())
        def dock_state():
            return tomllib.loads(ctl("dock", "state").stdout)
        if reveal:
            send(f"move {width // 2} {height - 1}")
            wait(lambda: dock_state()["visible"])
        dock = wait(lambda: (state if (state := dock_state())["height"] > 60 else None))
        x = (width - dock["width"]) // 2 + 52
        y = height - 12 - dock["height"] + 55
        send(f"move {x} {y}")
        time.sleep(.15)
        send("button 272 0")
        wait(lambda: "finished-copy=" in path.read_text())
        source.terminate()
        source.wait(timeout=3)
        return path.read_text()

    send("move 20 100")
    initial = config()
    original_file = installed.read_bytes()
    finished = drag("desktop-valid", [installed])
    saved = wait(lambda: (current if (current := config())["pins"][0] == "tools-editor.desktop" else None))
    assert saved["pins"] == ["tools-editor.desktop", *initial["pins"]]
    assert tomllib.loads(config_file.read_text())["pins"] == saved["pins"]
    assert "dnd-finished" in finished and "finished-copy=true" in finished
    assert installed.read_bytes() == original_file and not marker.exists()

    outside = config_file.parent / "editor.desktop"
    outside.write_bytes(original_file)
    assert "dnd-cancelled" in drag("desktop-untrusted-copy", [outside])
    assert config() == saved and outside.read_bytes() == original_file
    drag("desktop-duplicate", [installed])
    assert config() == saved
    assert "dnd-cancelled" in drag("desktop-multiple", [installed, outside])
    assert config() == saved
    send("move 20 100")
    ctl("config", "set", "dock.auto_hide=true")
    wait(lambda: not tomllib.loads(ctl("dock", "state").stdout)["visible"])
    assert "dnd-finished" in drag("desktop-autohide", [installed], reveal=True)
    ctl("config", "set", "dock.auto_hide=false")

    managed = config_file.with_name("managed.toml")
    config_file.rename(managed)
    config_file.symlink_to(managed)
    ctl("reload")
    try:
        assert "dnd-cancelled" in drag("desktop-managed", [installed])
        assert tomllib.loads(managed.read_text())["pins"] == saved["pins"]
    finally:
        config_file.unlink()
        managed.rename(config_file)
        ctl("reload")
    assert not marker.exists()

    pointer.stdin.write("quit\n")
    pointer.stdin.flush()
    assert pointer.wait(timeout=3) == 0
    result = {"passed": True, "external_drag_uses_desktop_id": True, "inserted_before_pin": True,
              "read_only_copy_action": True, "ignored_duplicate": True, "rejected_untrusted_copy": True,
              "rejected_multiple": True, "persisted_without_launch": True,
              "drag_reveals_hidden_dock": True, "managed_config_rejected": True}
    (report / "desktop-drop.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
