"""Test sound commands and the open control center on a private PulseAudio server."""

import json
from pathlib import Path
import subprocess
import tomllib


def start(args, env, root, children, logs, wait):
    socket = root / "pulse-native"
    env["PULSE_SERVER"] = f"unix:{socket}"
    env["PULSE_STATE_PATH"] = str(root / "pulse-state")
    env["PULSE_COOKIE"] = str(root / "pulse-cookie")
    log = (root / f"pulse-{len(children)}.log").open("w+")
    logs.append(log)
    server = subprocess.Popen(
        [
            args.pulseaudio, "-n", "--daemonize=no", "--exit-idle-time=-1",
            "--use-pid-file=no", "--log-target=stderr",
            "-L", f"module-native-protocol-unix socket={socket} auth-anonymous=1",
            "-L", "module-null-sink sink_name=mosaic_alpha sink_properties=device.description=Alpha",
            "-L", "module-null-sink sink_name=mosaic_beta sink_properties=device.description=Beta",
            "-L", "module-remap-source master=mosaic_alpha.monitor source_name=mosaic_mic",
        ],
        env=env, stdout=log, stderr=log,
    )
    children.append(server)
    wait(lambda: subprocess.run(
        [args.pactl, "info"], env=env, capture_output=True, timeout=3
    ).returncode == 0)
    return server


def run(args, env, root, children, logs, ctl, wait, server):
    def status():
        return tomllib.loads(ctl("audio", "status").stdout)

    def pactl(*cmd):
        return subprocess.check_output([args.pactl, *cmd], env=env, text=True, timeout=3).strip()

    def apply(action, name, value):
        ctl("audio", "set", str(status()["revision"]), action, name, str(value))

    def device(group, name):
        return next(d for d in status()[group] if d["name"] == name)

    def image():
        return subprocess.check_output(
            [args.grim, "-g", "435,260 405x260", "-"], env=env, timeout=3
        )

    def keys(*key_names):
        command = [args.wtype, "-s", "100"]
        for key in key_names:
            command.extend(["-k", key])
        subprocess.run(command, env=env, check=True, timeout=5)

    wait(lambda: status()["available"])
    apply("default-sink", "mosaic_beta", 0)
    wait(lambda: status()["default_sink"] == "mosaic_beta")
    apply("default-source", "mosaic_mic", 0)
    wait(lambda: status()["default_source"] == "mosaic_mic")
    assert pactl("get-default-sink") == "mosaic_beta"
    assert pactl("get-default-source") == "mosaic_mic"

    pactl("set-sink-volume", "mosaic_beta", "50%", "100%")
    wait(lambda: device("sinks", "mosaic_beta")["channels"] == [32768, 65536])
    apply("sink-volume", "mosaic_beta", 44)
    wait(lambda: device("sinks", "mosaic_beta")["volume"] == 44)
    channels = device("sinks", "mosaic_beta")["channels"]
    assert abs(channels[0] * 2 - channels[1]) <= 1
    assert "44%" in pactl("get-sink-volume", "mosaic_beta")
    apply("source-mute", "mosaic_mic", 1)
    assert pactl("get-source-mute", "mosaic_mic") == "Mute: yes"
    apply("source-volume", "mosaic_mic", 32)
    assert "32%" in pactl("get-source-volume", "mosaic_mic")
    assert device("sources", "mosaic_mic")["muted"]
    pactl("set-source-volume", "mosaic_mic", "55%")
    wait(lambda: device("sources", "mosaic_mic")["volume"] == 55)

    apply("sink-balance", "mosaic_beta", 125)
    adjusted = device("sinks", "mosaic_beta")
    assert adjusted["balance"] == 125
    assert adjusted["channels"][1] == max(channels)
    assert abs(adjusted["channels"][0] * 4 - adjusted["channels"][1] * 3) <= 2

    before = status()["revision"]
    pactl("set-sink-volume", "mosaic_beta", "71%")
    stale = ctl("audio", "set", str(before), "sink-volume", "mosaic_beta", "36", ok=False)
    assert stale.returncode and "Conflict" in stale.stderr
    wait(lambda: device("sinks", "mosaic_beta")["volume"] == 71)
    assert "71%" in pactl("get-sink-volume", "mosaic_beta")
    assert ctl("audio", "set", str(status()["revision"]), "sink-volume",
               "mosaic_beta", "101", ok=False).returncode

    pactl("set-sink-mute", "mosaic_beta", "0")
    wait(lambda: not device("sinks", "mosaic_beta")["muted"])
    ctl("control-center")
    wait(lambda: tomllib.loads(ctl("dialog", "state").stdout)["kind"] == "control-center")
    # Initial focus is on the theme. Keep appearance navigation covered by
    # the separate control-center scenario.
    keys(*(["Tab"] * 7), "space")
    wait(lambda: device("sinks", "mosaic_beta")["muted"])
    assert pactl("get-sink-mute", "mosaic_beta") == "Mute: yes"
    if args.grim:
        subprocess.run(
            [args.grim, "-o", "HEADLESS-1", str(Path(args.report).with_name("preview-audio.png"))],
            env=env, check=True, timeout=5,
        )
        before = image()
    pactl("set-sink-volume", "mosaic_beta", "29%")
    wait(lambda: device("sinks", "mosaic_beta")["volume"] == 29)
    if args.grim:
        wait(lambda: image() != before)

    def open_devices():
        ctl("dialog", "close")
        ctl("control-center")
        wait(lambda: tomllib.loads(ctl("dialog", "state").stdout)["kind"] == "control-center")

    before_balance = max(device("sinks", "mosaic_beta")["channels"])
    open_devices()
    keys(*(["Tab"] * 8), "Home")
    wait(lambda: device("sinks", "mosaic_beta")["balance"] == 0)
    assert device("sinks", "mosaic_beta")["channels"] == [before_balance, 0]
    assert pactl("get-default-sink") == "mosaic_beta"

    open_devices()
    keys(*(["Tab"] * 10), "Home")
    wait(lambda: device("sources", "mosaic_mic")["volume"] == 0)
    assert device("sources", "mosaic_mic")["muted"]
    if args.grim:
        subprocess.run(
            [args.grim, "-o", "HEADLESS-1", str(Path(args.report).with_name("preview-levels.png"))],
            env=env, check=True, timeout=5,
        )

    mono = pactl("load-module", "module-null-sink", "sink_name=mosaic_mono",
                 "channels=1", "channel_map=mono")
    wait(lambda: any(entry["name"] == "mosaic_mono" for entry in status()["sinks"]))
    assert "balance" not in device("sinks", "mosaic_mono")
    refused = ctl("audio", "set", str(status()["revision"]), "sink-balance",
                  "mosaic_mono", "50", ok=False)
    assert refused.returncode and "InvalidArgument" in refused.stderr
    pactl("unload-module", mono)

    server.terminate()
    server.wait(timeout=4)
    wait(lambda: not status()["available"])
    assert tomllib.loads(ctl("dialog", "state").stdout)["mapped"]
    assert ctl("audio", "set", str(status()["revision"]), "sink-volume",
               "mosaic_beta", "25", ok=False).returncode
    start(args, env, root, children, logs, wait)
    wait(lambda: status()["available"])
    assert len(status()["sinks"]) == 2
    assert tomllib.loads(ctl("dialog", "state").stdout)["mapped"]
    keys("Escape")
    wait(lambda: not tomllib.loads(ctl("dialog", "state").stdout)["mapped"])

    result = {
        "passed": True, "default_devices": True, "volume_preserves_balance": True,
        "input_mute": True, "gtk_output_mute": True, "external_volume": True,
        "conflict_refused": True, "server_restart_recovered": True,
        "gtk_stereo_balance": True, "gtk_input_level": True,
        "balance_preserves_peak": True, "mic_level_preserves_mute": True,
        "mono_balance_refused": True,
    }
    (Path(args.report).parent / "audio.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
