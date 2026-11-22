"""Keep two distinct playback streams routed independently on virtual outputs."""

import json
from pathlib import Path
import subprocess
import tomllib


def run(args, env, children, ctl, wait):
    def status():
        return tomllib.loads(ctl("audio", "status").stdout)

    def stream(application):
        return next((s for s in status()["playback"] if s["application"] == application), None)

    def set_audio(action, target, value, *, ok=True):
        return ctl("audio", "set", str(status()["revision"]), action, str(target), str(value), ok=ok)

    def keys(*names):
        command = [args.wtype, "-s", "100"]
        for name in names:
            command.extend(["-k", name])
        subprocess.run(command, env=env, check=True, timeout=6)

    wait(lambda: status()["available"])
    def start(application):
        with open("/dev/zero", "rb") as silence:
            client = subprocess.Popen(
                [args.pacat, "--playback", "--raw", "--device=mosaic_alpha",
                 f"--client-name={application}", "--stream-name=Test playback"],
                env=env, stdin=silence, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
        children.append(client)
        wait(lambda: stream(application))
        return client

    game_name, music_name = "Mosaic Game", "Mosaic Music"
    game = start(game_name)
    music = start(music_name)
    game_id = stream(game_name)["index"]
    assert game_id != stream(music_name)["index"]
    assert stream(game_name)["output"] == stream(music_name)["output"] == "mosaic_alpha"

    def open_applications():
        ctl("control-center")
        wait(lambda: tomllib.loads(ctl("dialog", "state").stdout)["kind"] == "control-center")
        keys(*(["Tab"] * 4), "space")

    default_output = status()["default_sink"]
    open_applications()
    keys("Tab", "Return", "End", "Return")
    wait(lambda: stream(game_name)["output"] == "mosaic_beta")
    assert stream(music_name)["output"] == "mosaic_alpha"
    assert status()["default_sink"] == default_output
    if args.grim:
        subprocess.run(
            [args.grim, "-o", "HEADLESS-1",
             str(Path(args.report).with_name("preview-playback.png"))],
            env=env, check=True, timeout=4,
        )

    old_music = stream(music_name)
    ctl("dialog", "close")
    open_applications()
    keys("Tab", "Tab", "Home")
    wait(lambda: stream(game_name)["volume"] == 0)
    set_audio("stream-volume", game_id, 37)
    wait(lambda: stream(game_name)["volume"] == 37)
    ctl("dialog", "close")
    open_applications()
    keys("Tab", "Tab", "Tab", "space")
    wait(lambda: stream(game_name)["muted"])
    assert stream(music_name) == old_music

    # The audio server can also move the stream while the panel is open.
    subprocess.run(
        [args.pactl, "move-sink-input", str(game_id), "mosaic_alpha"],
        env=env, check=True, timeout=4,
    )
    wait(lambda: stream(game_name)["output"] == "mosaic_alpha")
    game.terminate()
    game.wait(timeout=4)
    wait(lambda: stream(game_name) is None)
    removed = set_audio("stream-output", "mosaic_beta", game_id, ok=False)
    assert removed.returncode != 0 and "InvalidArgument" in removed.stderr
    assert stream(music_name) == old_music
    ctl("dialog", "close")
    music.terminate()
    music.wait(timeout=4)
    wait(lambda: not status()["playback"])

    result = {
        "passed": True, "gtk_routes_one_application": True,
        "other_application_unchanged": True, "independent_volume_and_mute": True,
        "external_route_observed": True, "closed_stream_rejected": True,
    }
    (Path(args.report).parent / "playback.json").write_text(
        json.dumps(result, indent=2) + "\n"
    )
    print(json.dumps(result), flush=True)
