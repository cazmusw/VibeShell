"""Deterministic pactl responses for testing hardware-specific controls."""

import json
import os
from pathlib import Path
import sys
import time


card_name = "alsa_card.mosaic_test"
speaker_name = "alsa_output.mosaic_test"
microphone_name = "alsa_input.mosaic_test"
duplex = "output:analog-stereo+input:analog-stereo"
output_only = "output:analog-stereo"
headphones = "analog-output-headphones"
speakers = "analog-output-speaker"
internal_mic = "analog-input-internal-mic"
external_mic = "analog-input-mic"


def initial_state(path):
    path.write_text(json.dumps({
        "profile": duplex, "sink_port": speakers, "source_port": internal_mic,
        "headphones_available": True,
    }))


def get_state(path):
    return json.loads(path.read_text())


def store(path, state):
    temporary = path.with_suffix(f".{os.getpid()}.tmp")
    temporary.write_text(json.dumps(state))
    temporary.replace(path)


def port(name, description, available=True):
    return {"name": name, "description": description,
            "availability": "availability unknown" if available else "not available"}


def serve(path, arguments):
    state = get_state(path)
    if arguments == ["subscribe"]:
        before = path.read_text()
        while True:
            time.sleep(.05)
            after = path.read_text()
            if after != before:
                print("Event 'change' on card #1", flush=True)
                before = after
    if arguments == ["info"]:
        return {"default_sink_name": speaker_name,
                "default_source_name": microphone_name if state["profile"] == duplex else ""}
    if arguments == ["list", "cards"]:
        return [{
            "name": card_name, "active_profile": state["profile"],
            "properties": {"device.description": "Test audio card"},
            "profiles": {
                duplex: {"description": "Duplex", "priority": 60, "available": True},
                output_only: {"description": "Analog output", "priority": 50, "available": True},
                "output:hdmi-stereo": {"description": "HDMI disconnected", "priority": 40, "available": False},
            },
        }]
    if arguments == ["list", "sinks"]:
        return [{"name": speaker_name, "description": "Test output", "mute": False,
                 "active_port": state["sink_port"],
                 "ports": [port(speakers, "Speakers"),
                           port(headphones, "Headphones", state["headphones_available"])]}]
    if arguments == ["list", "sources"]:
        if state["profile"] != duplex:
            return []
        return [{"name": microphone_name, "description": "Test input", "mute": False,
                 "active_port": state["source_port"],
                 "ports": [port(internal_mic, "Internal mic"), port(external_mic, "External mic")]}]
    if arguments == ["list", "sink-inputs"]:
        return []

    match arguments:
        case ["set-card-profile", target, name] if target == card_name and name in (duplex, output_only):
            state["profile"] = name
        case ["set-sink-port", target, name] if target == speaker_name and name in (speakers, headphones):
            if name == headphones and not state["headphones_available"]:
                raise SystemExit(1)
            state["sink_port"] = name
        case ["set-source-port", target, name] if target == microphone_name and name in (internal_mic, external_mic):
            if state["profile"] != duplex:
                raise SystemExit(1)
            state["source_port"] = name
        case _:
            raise SystemExit(2)
    store(path, state)
    return None


if __name__ == "__main__":
    result = serve(Path(os.environ["MOSAIC_HARDWARE_FIXTURE"]), sys.argv[1:])
    if result is not None:
        print(json.dumps(result))
