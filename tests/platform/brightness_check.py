"""Exercise local brightness through logind and GTK on an isolated bus."""

import json
from pathlib import Path
import select
import subprocess
import time
import tomllib


RAW = "intel_backlight"
FIRMWARE = "acpi_video0"
MAX_RAW = 937
LOGIND = "org.freedesktop.login1"
FIXTURE = "org.mosaic.BrightnessFixture"
SERVICES = "org.mosaic.Services1"


def prepare(env, root):
    backlights = root / "backlight"
    backlights.mkdir()
    env["MOSAIC_BRIGHTNESS_FIXTURE_ROOT"] = str(backlights)
    env["DBUS_SYSTEM_BUS_ADDRESS"] = env["DBUS_SESSION_BUS_ADDRESS"]
    for name, kind, level, maximum in (
        (FIRMWARE, "firmware", 7, 15),
        (RAW, "raw", 375, MAX_RAW),
        ("invalid_max", "raw", 1, 0),
        ("invalid_level", "raw", 10, 7),
        ("unsafe name", "raw", 30, 100),
    ):
        device = backlights / name
        device.mkdir()
        for entry, value in (
            ("type", kind), ("max_brightness", maximum), ("brightness", level),
            ("actual_brightness", level),
        ):
            (device / entry).write_text(f"{value}\n", encoding="ascii")


def run(args, env, children, ctl, wait, spawn):
    backlights = Path(env["MOSAIC_BRIGHTNESS_FIXTURE_ROOT"])
    raw = backlights / RAW
    output = "HEADLESS-1"

    def call(name, path, interface, method, *parameters, ok=True):
        completed = subprocess.run(
            ["busctl", "--user", "--json=short", "call", name, path, interface,
             method, *parameters],
            env=env, capture_output=True, text=True, timeout=5,
        )
        if ok and completed.returncode:
            raise RuntimeError(completed.stderr)
        return completed

    def fixture_call(method, *parameters):
        completed = call(LOGIND, "/org/mosaic/BrightnessFixture", FIXTURE, method, *parameters)
        if completed.stdout:
            return json.loads(completed.stdout)["data"]

    def status():
        completed = call(SERVICES, "/org/mosaic/Services1", SERVICES, "GetBrightness")
        return tomllib.loads(json.loads(completed.stdout)["data"][0])

    def level(name=RAW):
        return int((backlights / name / "actual_brightness").read_text(encoding="ascii"))

    def device(name=RAW):
        return next(item for item in status()["devices"] if item["name"] == name)

    def set_brightness(owner, session, name, identity, maximum, percent, ok=True):
        return call(SERVICES, "/org/mosaic/Services1", SERVICES, "SetBrightness", "ssssuu",
                    owner, session, name, identity, str(maximum), str(percent), ok=ok)

    def close():
        ctl("dialog", "close")
        wait(lambda: not dialog()["mapped"])

    def dialog():
        return tomllib.loads(ctl("dialog", "state").stdout)

    def showing(kind):
        info = dialog()
        return info["mapped"] and info["kind"] == kind and info["connector"] == output

    def screenshot(name):
        if args.grim:
            subprocess.run(
                [args.grim, "-o", output, str(Path(args.report).with_name(name))],
                env=env, check=True, timeout=5,
            )

    def pane_image():
        return subprocess.check_output(
            [args.grim, "-g", "415,122 450x390", "-"], env=env, timeout=5,
        )

    def keys(*names):
        command = [args.wtype, "-s", "100"]
        for name in names:
            command.extend(["-k", name])
        subprocess.run(command, env=env, check=True, timeout=5)

    assert status()["access"] == "no-service"
    fixture = spawn(args.brightness_fixture)
    initial = wait(lambda: (state := status())["access"] == "ready" and state)
    assert [item["name"] for item in initial["devices"]] == [RAW, FIRMWARE], initial
    primary, firmware = initial["devices"]
    raw_identity, firmware_identity = primary["identity"], firmware["identity"]
    assert raw_identity and firmware_identity and raw_identity != firmware_identity
    assert (primary["level"], primary["max"], primary["percent"]) == (375, MAX_RAW, 40)
    owner, session = initial["owner"], initial["session"]
    assert owner.startswith(":") and session
    assert fixture_call("Writes") == [0, 0, "", 0]

    set_brightness(owner, session, RAW, raw_identity, MAX_RAW, 75)
    assert device()["level"] == level() == 703 and device()["percent"] == 75
    assert fixture_call("Writes") == [1, 1, RAW, 703]
    assert level(FIRMWARE) == 7
    set_brightness(owner, session, FIRMWARE, firmware_identity, 15, 1)
    assert device(FIRMWARE)["level"] == level(FIRMWARE) == 1
    assert fixture_call("Writes") == [2, 2, FIRMWARE, 1]
    assert level() == 703

    for invalid in (
        (":999.999", session, RAW, raw_identity, MAX_RAW, 30),
        (owner, "stale-session", RAW, raw_identity, MAX_RAW, 30),
        (owner, session, RAW, firmware_identity, MAX_RAW, 30),
        (owner, session, RAW, raw_identity, MAX_RAW - 1, 30),
        (owner, session, "../intel_backlight", raw_identity, MAX_RAW, 30),
        (owner, session, "unsafe name", raw_identity, 100, 30),
        (owner, session, "missing", raw_identity, MAX_RAW, 30),
        (owner, session, RAW, raw_identity, MAX_RAW, 0),
        (owner, session, RAW, raw_identity, MAX_RAW, 101),
    ):
        refused = set_brightness(*invalid, ok=False)
        assert refused.returncode, invalid
    old_raw = backlights.parent / "original-intel-backlight"
    raw.rename(old_raw)
    raw.mkdir()
    try:
        for attribute in old_raw.iterdir():
            (raw / attribute.name).write_bytes(attribute.read_bytes())
        assert device()["identity"] != raw_identity and device()["max"] == MAX_RAW
        assert set_brightness(owner, session, RAW, raw_identity, MAX_RAW, 30, ok=False).returncode
    finally:
        for attribute in raw.iterdir():
            attribute.unlink()
        raw.rmdir()
        old_raw.rename(raw)
    assert fixture_call("Writes") == [2, 2, FIRMWARE, 1]

    pointer = subprocess.Popen(
        [args.pointer, "1280", "720"], env=env,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
    )
    children.append(pointer)

    def send(command):
        pointer.stdin.write(command + "\n")
        pointer.stdin.flush()
        assert select.select([pointer.stdout], [], [], 3)[0]
        assert pointer.stdout.readline().strip() == "ok"

    def click(x, y):
        send(f"move {x} {y}")
        time.sleep(.12)
        send("button 272 1")
        time.sleep(.06)
        send("button 272 0")

    ctl("control-center", output)
    wait(lambda: showing("control-center"))
    time.sleep(.4)
    click(530, 60)
    wait(lambda: showing("brightness"))
    time.sleep(.4)
    screenshot("preview-brightness.png")

    send("move 650 246")
    time.sleep(.12)
    send("axis v 15")
    wait(lambda: level() < 703)
    assert fixture_call("Writes")[:3] == [3, 3, RAW]
    assert device()["level"] == level()

    click(650, 246)
    keys("Home")
    wait(lambda: level() == 9)
    assert device()["percent"] == 1
    time.sleep(.3)
    before_keys = fixture_call("Writes")[0]
    keys("Right", "Right", "Right")
    wait(lambda: level() == 37)
    assert fixture_call("Writes")[0] == before_keys + 1
    keys("End")
    wait(lambda: level() == MAX_RAW)
    assert device()["percent"] == 100 and level(FIRMWARE) == 1

    time.sleep(.35)
    if args.grim:
        before = pane_image()
    (raw / "actual_brightness").write_text("0\n", encoding="ascii")
    wait(lambda: device()["level"] == 0 and device()["percent"] == 0)
    if args.grim:
        wait(lambda: pane_image() != before)
    assert int((raw / "brightness").read_text()) == MAX_RAW

    # Select a confirmed mid-range brightness, then refuse the next UI write.
    (raw / "actual_brightness").write_text("375\n", encoding="ascii")
    wait(lambda: device()["percent"] == 40)
    time.sleep(1.4)
    if args.grim:
        before = pane_image()
    accepted = fixture_call("Writes")
    fixture_call("SetDenied", "b", "true")
    send("axis v 15")
    wait(lambda: fixture_call("Writes")[0] == accepted[0] + 1)
    assert fixture_call("Writes")[1:] == accepted[1:]
    assert level() == 375 and device()["percent"] == 40
    if args.grim:
        wait(lambda: pane_image() != before)
    time.sleep(.35)
    blocked = fixture_call("Writes")
    send("axis v 15")
    keys("Home", "End")
    time.sleep(.5)
    assert fixture_call("Writes") == blocked and level() == 375
    screenshot("preview-brightness-denied.png")
    fixture_call("SetDenied", "b", "false")

    fixture_call("SetSession", "bb", "false", "false")
    wait(lambda: status()["access"] == "no-session")
    assert set_brightness(owner, session, RAW, raw_identity, MAX_RAW, 30, ok=False).returncode
    fixture_call("SetSession", "bb", "true", "true")
    assert status()["access"] == "no-session"
    fixture_call("SetSession", "bb", "true", "false")
    wait(lambda: status()["access"] == "ready")
    assert fixture_call("Writes") == blocked

    for name in (RAW, FIRMWARE):
        (backlights / name).rename(backlights.parent / name)
    try:
        missing = wait(lambda: (state := status())["access"] == "no-device" and state)
        assert not missing.get("devices")
        assert set_brightness(owner, session, RAW, raw_identity, MAX_RAW, 30, ok=False).returncode
        assert fixture_call("Writes") == blocked
        close()
        ctl("brightness", "show", output)
        wait(lambda: showing("brightness"))
        time.sleep(.7)
        screenshot("preview-brightness-unavailable.png")
        keys("Escape")
        wait(lambda: not dialog()["mapped"])
    finally:
        for name in (RAW, FIRMWARE):
            (backlights.parent / name).rename(backlights / name)

    fixture.terminate()
    fixture.wait(timeout=4)
    wait(lambda: status()["access"] == "no-service")
    spawn(args.brightness_fixture)
    wait(lambda: status()["access"] == "ready" and status()["owner"] != owner)
    assert set_brightness(owner, session, RAW, raw_identity, MAX_RAW, 30, ok=False).returncode
    assert fixture_call("Writes") == [0, 0, "", 0]

    pointer.stdin.write("quit\n")
    pointer.stdin.flush()
    assert pointer.wait(timeout=3) == 0
    result = {
        "passed": True,
        "backlight_discovery_and_order": True,
        "raw_rounding_and_nonzero_minimum": True,
        "actual_brightness_readback_and_external_zero": True,
        "stale_session_owner_and_maximum_refused": True,
        "same_name_device_replacement_refused": True,
        "invalid_device_and_percent_refused": True,
        "opens_from_control_center_and_cli": True,
        "gtk_scroll_and_keyboard": True,
        "rapid_key_edits_coalesced": True,
        "authorization_refusal_preserves_confirmed_value": True,
        "authorization_refusal_blocks_ui_writes": True,
        "inactive_and_remote_session_read_only": True,
        "missing_devices_and_logind_restart": True,
        "escape_closes": True,
    }
    Path(args.report).with_name("brightness.json").write_text(
        json.dumps(result, indent=2) + "\n", encoding="utf-8",
    )
    print(json.dumps(result), flush=True)
