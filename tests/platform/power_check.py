"""Check the power module against test-only services on a private bus."""

import json
from pathlib import Path
import select
import subprocess
import time
import tomllib


def run(args, env, ctl, wait, spawn):
    def status():
        return tomllib.loads(ctl("power", "status").stdout)

    def configure(profile, denied=False, present=True):
        subprocess.run(["busctl", "--user", "call", "org.mosaic.PowerFixture", "/Fixture",
                        "org.mosaic.PowerFixture", "Configure", "sbb", profile,
                        str(denied).lower(), str(present).lower()], env=env, check=True, timeout=4)

    def current():
        return status().get("profiles", {}).get("active")

    def configure_and_wait(profile, **flags):
        configure(profile, **flags)
        wait(lambda: current() == profile)

    assert status() == {"upower_available": False}
    fixture = spawn(args.power_fixture)
    snapshot = wait(lambda: state if "profiles" in (state := status()) and state["upower_available"] else None)
    assert snapshot["on_battery"] and snapshot["battery_present"]
    assert snapshot["battery"] == {"percent": 63, "state": "discharging", "seconds": 5400}
    owner = snapshot["profiles"]["owner"]
    assert set(snapshot["profiles"]["offered"]) == {"power-saver", "balanced"}
    assert ctl("power", "set", owner, "balanced", "performance", ok=False).returncode != 0
    assert ctl("power", "set", owner, "power-saver", "balanced", ok=False).returncode != 0
    ctl("power", "set", owner, "balanced", "power-saver")
    wait(lambda: current() == "power-saver")
    configure_and_wait("balanced", denied=True)
    rejected = ctl("power", "set", owner, "balanced", "power-saver", ok=False)
    assert rejected.returncode != 0 and "PermissionDenied" in rejected.stderr, rejected.stderr
    assert current() == "balanced"
    configure_and_wait("power-saver", present=False)
    assert status()["battery_present"] is False and "battery" not in status()
    configure_and_wait("balanced")

    ctl("power", "show", "HEADLESS-1")
    wait(lambda: (state := tomllib.loads(ctl("dialog", "state").stdout))["mapped"]
         and state["kind"] == "power" and state["connector"] == "HEADLESS-1")
    if args.grim:
        time.sleep(.4)
        subprocess.run([args.grim, "-o", "HEADLESS-1", Path(args.report).with_name("preview-power.png")],
                       env=env, check=True, timeout=5)
    with subprocess.Popen([args.pointer, "1280", "720"], env=env, stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, text=True, bufsize=1) as pointer:
        def send(command):
            pointer.stdin.write(command + "\n")
            pointer.stdin.flush()
            assert select.select([pointer.stdout], [], [], 3)[0]
            assert pointer.stdout.readline().strip() == "ok"

        try:
            send("move 640 343")
            time.sleep(.18)
            send("button 272 1")
            send("button 272 0")
            wait(lambda: current() == "power-saver")
        finally:
            pointer.stdin.write("quit\n")
            pointer.stdin.flush()
            assert pointer.wait(timeout=3) == 0
    ctl("dialog", "close")
    fixture.terminate()
    fixture.wait(timeout=3)
    wait(lambda: status() == {"upower_available": False})
    replacement = spawn(args.power_fixture)
    next_owner = wait(lambda: state.get("owner") if (state := status().get("profiles", {})).get("owner") != owner else None)
    assert next_owner and ctl("power", "set", owner, "balanced", "power-saver", ok=False).returncode != 0
    assert current() == "balanced"
    replacement.terminate()
    replacement.wait(timeout=3)
    result = {"passed": True, "battery_and_estimate": True, "battery_absent": True,
              "confirmed_profile": True, "denied_write_unchanged": True,
              "old_owner_and_state_rejected": True, "service_restart": True, "power_dialog": True,
              "profile_changes_from_gui": True}
    Path(args.report).with_name("power.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
