"""Exercise Wi-Fi discovery on a private D-Bus with a NetworkManager fixture."""

import json
from pathlib import Path
import select
import subprocess
import sys
import time
import tomllib


def run(args, env, children, ctl, wait, spawn):
    def status():
        return tomllib.loads(ctl("wifi", "status").stdout)

    def dialog():
        return tomllib.loads(ctl("dialog", "state").stdout)

    def set_enabled(value):
        subprocess.run(
            ["busctl", "--user", "call", "org.freedesktop.NetworkManager",
             "/org/freedesktop/NetworkManager", "org.mosaic.WifiFixture",
             "SetEnabled", "b", str(value).lower()],
            env=env, check=True, timeout=4,
        )

    def screenshot(name):
        if args.grim:
            subprocess.run(
                [args.grim, "-o", "HEADLESS-1", str(Path(args.report).with_name(name))],
                env=env, check=True, timeout=4,
            )

    assert not status()["available"]
    ctl("control-center")
    wait(lambda: dialog()["kind"] == "control-center")
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
        send("button 272 0")

    click(452, 60)
    wait(lambda: dialog()["kind"] == "wifi" and dialog()["mapped"])
    fixture = spawn(args.wifi_fixture)
    wait(lambda: status()["available"])
    state = status()
    assert [adapter["name"] for adapter in state["adapters"]] == ["wlan0", "wlan1"]
    main, unmanaged = state["adapters"]
    assert main["managed"] and not unmanaged["managed"]
    assert main["last_scan"] == -1
    assert [network["name"] for network in main["networks"]] == ["Mosaic Lab", "Guest"]
    assert main["networks"][0]["connected"] and main["networks"][0]["strength"] == 46
    assert main["networks"][1]["protection"] == "open"
    refused = ctl("wifi", "scan", unmanaged["path"], ok=False)
    assert refused.returncode and "InvalidArgument" in refused.stderr

    # The already open dialog picks up NetworkManager without reopening.
    time.sleep(2.1)
    screenshot("preview-wifi-before.png")
    click(801, 168)
    wait(lambda: status()["adapters"][0]["last_scan"] >= 0)
    repeated = ctl("wifi", "scan", main["path"], ok=False)
    assert repeated.returncode and "Conflict" in repeated.stderr
    assert any(network["name"] == "New Test Network"
               for network in status()["adapters"][0]["networks"])
    time.sleep(2.1)
    screenshot("preview-wifi-before-connect.png")

    previous = status()["adapters"][0]["active_connection"]
    refused = ctl("wifi", "disconnect", main["path"], previous + "0", ok=False)
    assert refused.returncode and "Conflict" in refused.stderr
    click(798, 243)
    wait(lambda: status()["adapters"][0]["state"] == 110)
    wait(lambda: not status()["adapters"][0]["active_connection"])
    time.sleep(2.1)
    screenshot("preview-wifi-disconnected.png")
    click(798, 295)
    waiting = wait(lambda: (adapter if (adapter := status()["adapters"][0])["state"] == 40 else None))
    assert not any(network["connected"] for network in waiting["networks"])
    wait(lambda: any(network["connected"] and network["name"] == "New Test Network"
                     for network in status()["adapters"][0]["networks"]))
    active = status()["adapters"][0]["active_connection"]
    assert active and active != previous
    absent = ctl("wifi", "connect", main["path"], "open", "Guest", ok=False)
    assert absent.returncode and "NoSavedProfile" in absent.stderr
    wrong_kind = ctl("wifi", "connect", main["path"], "open", "New Test Network", ok=False)
    assert wrong_kind.returncode and "Conflict" in wrong_kind.stderr
    assert status()["adapters"][0]["active_connection"] == active
    time.sleep(2.1)
    screenshot("preview-wifi.png")

    def join(name, mode, ok=True):
        ssid = name.encode()
        result = subprocess.run(
            ["busctl", "--user", "call", "org.mosaic.Services1", "/org/mosaic/Services1",
             "org.mosaic.Services1", "JoinWifi", "says", main["path"], str(len(ssid)),
             *map(str, ssid), mode],
            env=env, capture_output=True, text=True, timeout=8,
        )
        if ok:
            result.check_returncode()
        return result

    refused = join("Personal Test", "sae", ok=False)
    assert refused.returncode and "security changed" in refused.stderr
    click(798, 425)
    time.sleep(.7)
    screenshot("preview-wifi-profile.png")
    assert status()["adapters"][0]["active_connection"] == active
    click(615, 281)
    time.sleep(.4)
    assert status()["adapters"][0]["active_connection"] == active
    click(798, 425)
    time.sleep(.7)
    click(500, 281)
    time.sleep(.25)
    screenshot("preview-wifi-profile-pending.png")
    wait(lambda: status()["adapters"][0]["state"] == 60)
    wait(lambda: any(network["connected"] and network["name"] == "Personal Test"
                     for network in status()["adapters"][0]["networks"]))
    current = status()["adapters"][0]["active_connection"]
    assert current and current != active
    time.sleep(2.1)
    ctl("wifi", "disconnect", main["path"], current)
    wait(lambda: not status()["adapters"][0]["active_connection"])
    assert current not in join("Personal Test", "wpa-psk").stdout
    wait(lambda: any(network["connected"] and network["name"] == "Personal Test"
                     for network in status()["adapters"][0]["networks"]))
    time.sleep(2.1)
    click(798, 489)
    time.sleep(.7)
    screenshot("preview-wifi-agent-offer.png")
    click(500, 281)
    wait(lambda: status()["adapters"][0]["state"] == 60)
    failed = wait(lambda: (adapter if (adapter := status()["adapters"][0])["state"] == 30
                           and "activation_failure" in adapter else None))
    assert failed["activation_failure"]["reason"] == 7 and not failed["active_connection"]
    time.sleep(2.1)
    screenshot("preview-wifi-no-agent.png")
    subprocess.run(
        ["busctl", "--user", "call", "org.freedesktop.NetworkManager",
         "/org/freedesktop/NetworkManager", "org.mosaic.WifiFixture", "EnableAgent"],
        env=env, check=True, timeout=4,
    )
    click(798, 505)
    wait(lambda: any(network["connected"] and network["name"] == "Needs Agent"
                     for network in status()["adapters"][0]["networks"]))
    assert "activation_failure" not in status()["adapters"][0]

    set_enabled(False)
    wait(lambda: not status()["enabled"])
    assert not status()["adapters"][0]["networks"]
    assert ctl("wifi", "scan", main["path"], ok=False).returncode
    fixture.terminate()
    fixture.wait(timeout=4)
    wait(lambda: not status()["available"])
    assert dialog()["kind"] == "wifi"
    spawn(args.wifi_fixture)
    wait(lambda: status()["available"] and status()["enabled"])
    assert dialog()["kind"] == "wifi"
    subprocess.run([args.wtype, "-s", "100", "-k", "Escape"], env=env, check=True, timeout=5)
    wait(lambda: not dialog()["mapped"])
    ctl("wifi", "show", "HEADLESS-1")
    wait(lambda: dialog()["kind"] == "wifi" and dialog()["connector"] == "HEADLESS-1")
    time.sleep(.5)

    def profile_call(method, *args, ok=True):
        result = subprocess.run(
            ["busctl", "--user", "--json=short", "call", "org.mosaic.Services1",
             "/org/mosaic/Services1", "org.mosaic.Services1", method, *args],
            env=env, capture_output=True, text=True, timeout=8,
        )
        if ok:
            result.check_returncode()
        return result

    def profiles():
        return tomllib.loads(json.loads(profile_call("GetWifiProfiles").stdout)["data"][0])

    snapshot = profiles()
    expected_names = ["Enterprise Lab", "Office Backup"]
    assert [item["name"] for item in snapshot["profiles"]] == expected_names
    first, backup = snapshot["profiles"]
    assert first["ssid"] == backup["ssid"] == "Office 802.1X"
    assert first["uuid"] != backup["uuid"]
    assert not any(network["name"] == first["ssid"]
                   for network in status()["adapters"][0]["networks"])
    for owner, uuid, ssid in [
        (":999.999", first["uuid"], first["ssid"]),
        (snapshot["owner"], first["uuid"], "Changed SSID"),
        (snapshot["owner"], backup["uuid"], first["ssid"]),
    ]:
        result = profile_call(
            "ForgetWifi", "sssss", owner, first["path"], uuid, first["name"], ssid,
            ok=False,
        )
        assert result.returncode and ("restarted" in result.stderr or "profile changed" in result.stderr)
    assert profiles() == snapshot

    def open_profiles(*, reopened=False):
        wait(lambda: dialog()["kind"] == "wifi" and dialog()["mapped"])
        if reopened:
            # Move off the newly mapped surface before clicking to establish pointer focus.
            send("move 950 120")
            time.sleep(.25)
        click(710, 166)
        if reopened:
            # The same position is the refresh control once Profiles is visible.
            time.sleep(.5)
            click(710, 166)
        time.sleep(.8)

    open_profiles()
    screenshot("preview-wifi-profiles.png")
    click(690, 254)
    apps = Path(env["XDG_DATA_HOME"]) / "applications"
    marker = apps.parent / "editor-argv.json"
    assert not marker.exists()
    editor = apps.parent / "mock-editor"
    editor.write_text(
        f"#!{sys.executable}\nimport json, sys\nfrom pathlib import Path\n"
        f"Path({str(marker)!r}).write_text(json.dumps(sys.argv[1:]))\n"
    )
    editor.chmod(0o755)
    (apps / "nm-connection-editor.desktop").write_text(
        f"[Desktop Entry]\nType=Application\nName=Fixture Editor\nExec={editor}\n"
    )
    click(690, 254)
    wait(marker.exists)
    assert json.loads(marker.read_text()) == ["--edit", first["uuid"]]
    wait(lambda: not dialog()["mapped"])
    marker.unlink()
    ctl("wifi", "show", "HEADLESS-1")
    open_profiles(reopened=True)
    click(601, 166)
    wait(marker.exists)
    assert json.loads(marker.read_text()) == []
    wait(lambda: not dialog()["mapped"])
    ctl("wifi", "show", "HEADLESS-1")
    open_profiles(reopened=True)

    click(801, 254)
    time.sleep(.5)
    screenshot("preview-wifi-forget.png")
    click(470, 308)
    assert dialog()["mapped"] and dialog()["kind"] == "wifi"
    assert profiles() == snapshot
    click(801, 254)
    time.sleep(.5)
    click(579, 308)
    wait(lambda: profiles()["profiles"] == [backup])
    remaining = subprocess.check_output(
        ["busctl", "--user", "call", "org.freedesktop.NetworkManager",
         "/org/freedesktop/NetworkManager/Settings", "org.freedesktop.NetworkManager.Settings",
         "ListConnections"],
        env=env, text=True, timeout=4,
    )
    assert "/Settings/21" in remaining and backup["path"] in remaining
    time.sleep(.6)
    screenshot("preview-wifi-forgotten.png")
    ctl("dialog", "close")

    pointer.stdin.write("quit\n")
    pointer.stdin.flush()
    assert pointer.wait(timeout=3) == 0
    result = {
        "passed": True, "opens_from_control_center": True,
        "adapter_inventory": True, "connection_and_deduplication": True,
        "gtk_scan": True, "scan_cooldown": True, "unmanaged_and_disabled_refused": True,
        "networkmanager_restart": True, "cli_opens_wifi": True,
        "gtk_disconnect": True, "gtk_connect_saved_network": True,
        "confirmation_waits_for_activation": True, "stale_disconnect_refused": True,
        "missing_profile_refused": True, "security_mismatch_refused": True,
        "gtk_personal_profile": True, "agent_owned_password": True,
        "unsupported_personal_refused": True, "missing_agent_reported": True,
        "saved_personal_profile_reused": True,
        "brief_failure_preserved": True, "saved_profile_reused_after_missing_agent": True,
        "offline_profiles_listed": True, "stale_forget_refused": True,
        "profile_editor_uuid": True, "external_editor": True,
        "cancelled_forget_preserved_profile": True, "gtk_forget_exact_profile": True,
    }
    (Path(args.report).parent / "wifi.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
