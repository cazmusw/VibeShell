"""Exercise slow, background, repeated and failed launches in a private session."""

import time


def prepare(apps, root, python, probe):
    delayed = root / "delayed-launch.py"
    delayed.write_text(
        "from pathlib import Path\nimport os, sys, time\n"
        f"marker = Path({str(root / 'started')!r})\n"
        "if not marker.exists():\n    marker.write_text('reuse')\n    time.sleep(11.5)\n"
        "elif marker.read_text() == 'reuse':\n    sys.exit(0)\n"
        f"os.execv({probe!r}, [{probe!r}, '--hold-window'])\n"
    )
    for name, command in [
        ("mosaic-stage-0-probe", f'"{python}" "{delayed}"'),
        ("background-launch", f'"{python}" -c "pass"'),
    ]:
        (apps / f"{name}.desktop").write_text(
            f"[Desktop Entry]\nType=Application\nName={name}\nExec={command}\n"
        )
    (apps / "broken-launch.desktop").write_text(
        "[Desktop Entry]\nType=Application\nName=Broken working directory\n"
        f'Exec="{python}" -c "pass"\nPath={root / "missing-directory"}\n'
    )


def run(ctl, wait, fixture, root):
    def logs():
        return ctl("logs").stdout

    unobserved = "\tshell\tlauncher_window_unobserved\t0\n"
    failed = "\tshell\tlauncher_failed\t1\n"
    old_warning = "\tshell\tlauncher_window_timeout\t1\n"
    ctl("launch", "mosaic-stage-0-probe.desktop")
    ctl("launch", "background-launch.desktop")
    wait(lambda: logs().count(unobserved) >= 2 or old_warning in logs())
    assert logs().count(unobserved) == 2, logs()
    assert failed not in logs()
    first = wait(fixture)

    ctl("launch", "mosaic-stage-0-probe.desktop")
    ctl("launch", "broken-launch.desktop")
    wait(lambda: failed in logs())
    time.sleep(10.3)
    assert wait(fixture)[:2] == first[:2]
    assert logs().count(unobserved) == 2, logs()
    assert old_warning not in logs()
    (root / "started").write_text("allow-relaunch")
