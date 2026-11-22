"""Send a terminal interrupt to Mosaic while its independently launched apps run."""

import errno
import json
import os
from pathlib import Path
import pty
import signal


def desktop_files(apps, root, python, probe):
    launcher = root / "window-process.py"
    launcher.write_text(
        "import os, sys\nfrom pathlib import Path\n"
        "Path(sys.argv[1]).write_text(str(os.getpid()))\n"
        "os.execv(sys.argv[2], [sys.argv[2], '--hold-window'])\n"
    )
    for name in ("alpha", "beta"):
        (apps / f"mosaic-interrupt-{name}.desktop").write_text(
            f'[Desktop Entry]\nType=Application\nName=Interrupt fixture {name}\n'
            f'Exec="{python}" "{launcher}" "{root / (name + ".pid")}" "{probe}"\n'
        )


def run(args, env, root, ctl, wait, windows, ui_ready, backend_ready):
    runner, terminal = pty.fork()
    if runner == 0:
        os.execve(args.session, [args.session], env)

    reaped = False

    def finished():
        nonlocal reaped
        pid, status = os.waitpid(runner, os.WNOHANG)
        reaped = pid != 0
        return (status,) if reaped else None

    try:
        wait(ui_ready)
        wait(backend_ready)
        for name in ("alpha", "beta"):
            ctl("launch", f"mosaic-interrupt-{name}.desktop")
        markers = [root / f"{name}.pid" for name in ("alpha", "beta")]
        wait(lambda: all(path.is_file() and path.stat().st_size for path in markers))
        processes = [int(marker.read_text()) for marker in markers]
        wait(lambda: len([w for w in windows() if w[2] == '"mosaic-stage-0-probe"']) == 2)
        inherited = [pid for pid in processes if os.getpgid(pid) == runner]
        os.write(terminal, b"\x03")
        (status,) = wait(finished, seconds=5)
        assert os.waitstatus_to_exitcode(status) == 130, status
        wait(lambda: not ui_ready() and not backend_ready())
        assert not inherited, f"applications shared terminal foreground process group: {inherited}"
        for pid in processes:
            # A zombie can still answer kill(pid, 0).
            contents = Path(f"/proc/{pid}/stat").read_text()
            assert contents.rsplit(")", 1)[1].split()[0] not in {"Z", "X"}
    finally:
        if not reaped:
            try:
                os.kill(runner, signal.SIGTERM)
            except OSError as error:
                if error.errno != errno.ESRCH:
                    raise
            wait(finished, seconds=5)
        os.close(terminal)

    result = {"passed": True, "terminal_interrupt_exit_130": True, "both_applications_survived": True}
    (Path(args.report).parent / "interrupt-session.json").write_text(
        json.dumps(result, indent=2) + "\n"
    )
    print(json.dumps(result), flush=True)
