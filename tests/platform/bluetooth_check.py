"""Drive the Bluetooth pane, discovery leases, and pairing on a private bus."""

import json
from pathlib import Path
import select
import subprocess
import time
import tomllib


def run(args, env, children, ctl, wait, spawn):
    def status():
        return tomllib.loads(ctl("bluetooth", "status").stdout)

    def dialog():
        return tomllib.loads(ctl("dialog", "state").stdout)

    def bus_call(name, path, interface, method, *parameters, ok=True):
        result = subprocess.run(
            ["busctl", "--user", "--json=short", "call", name, path, interface,
             method, *parameters],
            env=env, capture_output=True, text=True, timeout=8,
        )
        if ok:
            result.check_returncode()
        return result

    def action(method, *parameters, ok=True):
        return bus_call("org.mosaic.Services1", "/org/mosaic/Services1",
                        "org.mosaic.Services1", method, *parameters, ok=ok)

    def fixture_call(method, *parameters):
        return bus_call("org.bluez", "/org/mosaic/BluetoothFixture",
                        "org.mosaic.BluetoothFixture", method, *parameters)

    def data(result):
        values = json.loads(result.stdout)["data"]
        return values[0] if len(values) == 1 else values

    def counters():
        return data(fixture_call("Counters"))

    def device_counters():
        return data(fixture_call("DeviceCounters"))

    def pairing_state():
        return data(fixture_call("PairingState"))

    def screenshot(name):
        if args.grim:
            subprocess.run(
                [args.grim, "-o", "HEADLESS-1", str(Path(args.report).with_name(name))],
                env=env, check=True, timeout=4,
            )

    assert not status()["available"]
    ctl("control-center")
    wait(lambda: dialog()["kind"] == "control-center" and dialog()["mapped"])
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

    time.sleep(.4)
    click(780, 60)
    wait(lambda: dialog()["kind"] == "bluetooth" and dialog()["mapped"])
    fixture = spawn(args.bluetooth_fixture)
    wait(lambda: status()["available"])
    initial = status()
    assert initial["pairing_enabled"]
    first, second = initial["adapters"]
    assert not first["powered"] and second["powered"]
    assert first["devices"][0]["battery"] == 0 and first["devices"][0]["paired"]
    assert second["devices"][0]["name"] == "Second Radio Speaker"
    owner, adapter = initial["owner"], first["path"]
    refused = action("SetBluetoothPower", "ssb", ":999.999", adapter, "true", ok=False)
    assert refused.returncode and "restarted" in refused.stderr
    refused = action("ScanBluetooth", "ss", owner, adapter, ok=False)
    assert refused.returncode and "off" in refused.stderr

    def primary():
        return status()["adapters"][0]

    time.sleep(2.1)
    screenshot("preview-bluetooth-off.png")
    click(659, 166)
    wait(lambda: primary()["powered"] and primary()["devices"][0]["connected"])
    assert status()["adapters"][1] == second
    time.sleep(2.1)
    fixture_call("SetExternalSearch", "b", "true")
    click(793, 166)
    wait(lambda: primary()["searching"] and len(primary()["devices"]) == 2)
    assert counters() == [1, 0, 1]
    repeat = action("ScanBluetooth", "ss", owner, adapter, ok=False)
    assert repeat.returncode and "already running" in repeat.stderr
    time.sleep(2.1)
    screenshot("preview-bluetooth.png")
    click(793, 166)
    wait(lambda: not primary()["searching"] and counters() == [1, 1, 0])
    assert primary()["discovering"]
    time.sleep(2.1)

    click(793, 166)
    wait(lambda: primary()["searching"] and counters() == [2, 1, 1])
    ctl("dialog", "close")
    wait(lambda: not dialog()["mapped"])
    wait(lambda: not primary()["searching"] and counters() == [2, 2, 0], seconds=23)
    assert primary()["discovering"]

    action("ScanBluetooth", "ss", owner, adapter)
    assert counters() == [3, 2, 1]
    fixture_call("SetExternalPower", "b", "false")
    wait(lambda: not primary()["powered"] and not primary()["searching"])
    fixture_call("SetExternalPower", "b", "true")
    action("ScanBluetooth", "ss", owner, adapter)
    assert counters() == [4, 2, 1]
    action("StopBluetoothScan", "ss", owner, adapter)
    assert counters() == [4, 3, 0]

    keyboard_path = adapter + "/dev_00_11_22_33_44_66"
    keyboard_address = "00:11:22:33:44:66"

    def keyboard():
        return next((device for device in primary()["devices"]
                     if device["path"] == keyboard_path), {})

    refused = action("ChangeBluetoothDevice", "sssss", owner, adapter, keyboard_path,
                     "00:11:22:33:44:FF", "connect", ok=False)
    assert refused.returncode and "Bluetooth device changed" in refused.stderr, refused.stderr
    refused = action("ChangeBluetoothDevice", "sssss", owner, adapter, keyboard_path,
                     keyboard_address, "connect", ok=False)
    assert refused.returncode, "unpaired Bluetooth device was allowed to connect"
    assert device_counters() == [0] * 10

    def pairing_client(token):
        client = subprocess.Popen(
            [args.bluetooth_fixture, "--pair-client", owner, adapter, keyboard_path,
             keyboard_address, token], env=env, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, text=True,
        )
        children.append(client)
        assert select.select([client.stdout], [], [], 8)[0], "pairing prompt timed out"
        prompt, kind, code = client.stdout.readline().split()
        assert int(prompt) > 0 and kind == "confirm" and code == "004207"
        return client, prompt

    client, prompt = pairing_client("41")
    assert pairing_state() == [True, 1]
    assert data(action("GetBluetoothPairing", "t", "41")) == [0, "", ""]
    refused = action("RespondBluetoothPairing", "ttbs", "41", prompt, "true", "", ok=False)
    assert refused.returncode, "foreign caller answered the Bluetooth prompt"
    action("CancelBluetoothPairing", "t", "41")
    assert pairing_state() == [True, 1]
    refused = action("PairBluetooth", "sssst", owner, adapter, keyboard_path, keyboard_address,
                     "99", ok=False)
    assert refused.returncode and "already in progress" in refused.stderr, refused.stderr
    client.communicate("reject\n", timeout=8)
    assert client.returncode == 0
    assert device_counters() == [1, 1, 1, 0, 1, 0, 0, 0, 0, 0]
    assert pairing_state() == [False, 0] and not keyboard()["paired"]

    client, _ = pairing_client("42")
    client.terminate()
    client.wait(timeout=4)
    wait(lambda: pairing_state() == [False, 0])
    assert device_counters() == [2, 1, 2, 0, 1, 1, 0, 0, 0, 0]
    assert not keyboard()["paired"]

    ctl("bluetooth", "show", "HEADLESS-1")
    wait(lambda: dialog()["kind"] == "bluetooth" and dialog()["mapped"])
    time.sleep(.4)
    click(823, 285)  # Сопрячь
    wait(lambda: pairing_state() == [True, 1])
    ctl("dialog", "close")
    wait(lambda: not dialog()["mapped"] and pairing_state() == [False, 0])
    assert device_counters() == [3, 1, 3, 0, 1, 2, 0, 0, 0, 0]
    assert not keyboard()["paired"]

    ctl("bluetooth", "show", "HEADLESS-1")
    wait(lambda: dialog()["kind"] == "bluetooth" and dialog()["mapped"])
    time.sleep(.4)
    click(823, 285)  # Сопрячь
    wait(lambda: pairing_state() == [True, 1])
    time.sleep(.6)
    click(707, 273)  # Отклонить
    wait(lambda: pairing_state() == [False, 0])
    assert device_counters() == [4, 2, 4, 0, 2, 2, 0, 0, 0, 0]
    assert not keyboard()["paired"]

    time.sleep(.4)
    click(823, 285)  # Сопрячь
    wait(lambda: pairing_state() == [True, 1])
    time.sleep(.6)
    screenshot("preview-bluetooth-pairing.png")
    click(804, 273)  # Подтвердить
    wait(lambda: keyboard().get("paired") and pairing_state() == [False, 0])
    assert not keyboard()["connected"]
    trusted = bus_call("org.bluez", keyboard_path, "org.freedesktop.DBus.Properties", "Get",
                       "ss", "org.bluez.Device1", "Trusted")
    assert data(trusted) == {"type": "b", "data": False}
    fixture_call("SetConcurrentConnect", "b", "true")
    connect_args = ("sssss", owner, adapter, keyboard_path, keyboard_address, "connect")
    reconnect = action("ChangeBluetoothDevice", *connect_args, ok=False)
    assert reconnect.returncode == 0, reconnect.stderr
    assert keyboard()["connected"]
    action("ChangeBluetoothDevice", *connect_args)
    action("ChangeBluetoothDevice", "sssss", owner, adapter, keyboard_path,
           keyboard_address, "disconnect")
    fixture_call("SetConcurrentConnect", "b", "false")
    stalled = action("ChangeBluetoothDevice", *connect_args, ok=False)
    assert stalled.returncode and "still did not connect" in stalled.stderr, stalled.stderr
    assert not keyboard()["connected"]
    time.sleep(.3)
    click(723, 285)  # Подключить
    wait(lambda: keyboard().get("connected"))
    time.sleep(.3)
    click(723, 236)  # Отключить, the connected keyboard sorts first
    wait(lambda: not keyboard().get("connected"))
    time.sleep(.3)
    click(823, 285)  # Удалить
    time.sleep(.3)
    assert keyboard().get("paired") and device_counters()[8] == 0
    screenshot("preview-bluetooth-remove.png")
    click(737, 273)  # Отмена
    time.sleep(.3)
    click(823, 285)  # Удалить
    time.sleep(.3)
    click(822, 273)  # Удалить in confirmation
    wait(lambda: not keyboard())
    assert device_counters() == [5, 3, 5, 1, 2, 2, 2, 2, 1, 0]
    ctl("dialog", "close")
    wait(lambda: not dialog()["mapped"])

    fixture.terminate()
    fixture.wait(timeout=4)
    wait(lambda: not status()["available"])
    ctl("bluetooth", "show", "HEADLESS-1")
    wait(lambda: dialog()["kind"] == "bluetooth" and dialog()["mapped"]
         and dialog()["connector"] == "HEADLESS-1")
    spawn(args.bluetooth_fixture)
    wait(lambda: status()["available"] and status()["owner"] != owner)
    resumed = status()
    assert not resumed["adapters"][0]["powered"]
    refused = action("SetBluetoothPower", "ssb", owner, adapter, "true", ok=False)
    assert refused.returncode and "restarted" in refused.stderr
    time.sleep(.4)
    subprocess.run([args.wtype, "-s", "100", "-k", "Escape"], env=env, check=True, timeout=5)
    wait(lambda: not dialog()["mapped"])

    pointer.stdin.write("quit\n")
    pointer.stdin.flush()
    assert pointer.wait(timeout=3) == 0
    result = {
        "passed": True, "opens_from_control_center": True, "opens_from_cli": True,
        "adapter_inventory": True, "zero_battery": True, "gtk_adapter_power": True,
        "gtk_start_and_stop": True, "search_discovers_devices": True,
        "duplicate_and_unpowered_search_refused": True,
        "external_search_preserved": True, "lease_expires_with_pane_closed": True,
        "stale_owner_refused": True, "bluez_restart": True,
        "external_power_cycle_reconciled": True,
        "stale_device_address_refused": True, "unpaired_connect_refused": True,
        "scoped_agent_confirmation": True, "six_digit_passkey": True,
        "foreign_and_stale_responses_refused": True, "pairing_rejection": True,
        "pairing_cancelled_when_requester_leaves": True,
        "gtk_rejection_and_pane_close_cancellation": True,
        "gtk_pairing_without_auto_connect_or_trust": True,
        "gtk_connect_disconnect_and_confirm_removal": True,
        "concurrent_reconnect_confirmed": True, "stalled_reconnect_times_out": True,
        "opt_in_pairing_enabled": True, "incoming_agent_request_rejected": True,
    }
    Path(args.report).with_name("bluetooth.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
