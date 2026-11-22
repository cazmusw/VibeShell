"""Exercise hardware profile and port selections with an isolated pactl fixture."""

import json
from pathlib import Path
import subprocess
import tomllib

import hardware_pactl as model


def run(args, env, path, ctl, wait):
    def status():
        return tomllib.loads(ctl("audio", "status").stdout)

    def command(action, name, index, *, revision=None, ok=True):
        revision = status()["revision"] if revision is None else revision
        return ctl("audio", "set", str(revision), action, name, str(index), ok=ok)

    def index(options, name):
        return next(i for i, option in enumerate(options) if option["name"] == name)

    def keys(*names):
        command_line = [args.wtype, "-s", "120"]
        for name in names:
            command_line.extend(["-k", name])
        subprocess.run(command_line, env=env, check=True, timeout=6)

    def close():
        ctl("dialog", "close")
        wait(lambda: not tomllib.loads(ctl("dialog", "state").stdout)["mapped"])

    def open_hardware():
        ctl("control-center")
        wait(lambda: tomllib.loads(ctl("dialog", "state").stdout)["kind"] == "control-center")
        keys(*(["Tab"] * 5), "space")

    wait(lambda: status()["cards"])
    card = status()["cards"][0]
    unavailable = command("card-profile", model.card_name,
                          index(card["profiles"], "output:hdmi-stereo"), ok=False)
    assert unavailable.returncode and "InvalidArgument" in unavailable.stderr

    open_hardware()
    keys("Tab", "Return", "End", "Return")
    wait(lambda: status()["cards"][0]["active_profile"] == model.output_only)
    assert not status()["sources"]
    assert model.get_state(path)["profile"] == model.output_only

    close()
    open_hardware()
    keys("Tab", "Tab", "Return", "Home", "Return")
    wait(lambda: status()["sinks"][0]["active_port"] == model.headphones)
    assert model.get_state(path)["sink_port"] == model.headphones
    command("card-profile", model.card_name, index(card["profiles"], model.duplex))
    wait(lambda: status()["sources"])
    source = status()["sources"][0]
    command("source-port", model.microphone_name, index(source["ports"], model.external_mic))
    wait(lambda: status()["sources"][0]["active_port"] == model.external_mic)
    assert model.get_state(path)["source_port"] == model.external_mic

    if args.grim:
        subprocess.run(
            [args.grim, "-o", "HEADLESS-1",
             str(Path(args.report).with_name("preview-hardware.png"))],
            env=env, check=True, timeout=4,
        )
    stale = status()["revision"]
    output = status()["sinks"][0]
    state = model.get_state(path)
    state["headphones_available"] = False
    state["sink_port"] = model.speakers
    model.store(path, state)
    wait(lambda: status()["sinks"][0]["active_port"] == model.speakers)
    refused = command("sink-port", model.speaker_name,
                      index(output["ports"], model.headphones), revision=stale, ok=False)
    assert refused.returncode and "Conflict" in refused.stderr
    close()
    result = {
        "passed": True, "gtk_card_profile": True, "gtk_output_port": True,
        "source_port": True, "profile_updates_devices": True,
        "unavailable_profile_refused": True, "external_port_change_observed": True,
        "stale_port_refused": True,
    }
    (Path(args.report).parent / "hardware.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
