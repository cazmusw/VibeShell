"""Click the panel clock and dismiss the calendar on a private Wayland seat."""
import json
from pathlib import Path
import select
import subprocess
import tomllib


def run(args, env, processes, logs, ctl, wait):
    directory = Path(args.report).parent
    log = (directory / "calendar-pointer.log").open("w+")
    logs.append(log)
    pointer = subprocess.Popen(
        [args.pointer, "1280", "720"], env=env,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log,
        text=True, bufsize=1,
    )
    processes.append(pointer)

    def send(command):
        pointer.stdin.write(command + "\n")
        pointer.stdin.flush()
        assert select.select([pointer.stdout], [], [], 3)[0]
        assert pointer.stdout.readline().strip() == "ok"

    def click(x, y):
        send(f"move {x} {y}")
        send("button 272 1")
        send("button 272 0")

    def image(region="870,45 410x235"):
        return subprocess.check_output(
            [args.grim, "-g", region, "-"], env=env, timeout=5,
        )

    before = image()
    month_region = "990,94 288x164"
    before_month = image(month_region)
    click(1213, 16)
    opened = wait(lambda: (pixels if (pixels := image()) != before else None))
    subprocess.run(
        [args.grim, "-o", "HEADLESS-1", str(directory / "preview-calendar.png")],
        env=env, check=True, timeout=5,
    )
    current_month = image(month_region)
    click(1014, 70)
    send("move 1186 16")
    wait(lambda: image(month_region) != current_month)
    click(1133, 287)
    send("move 1186 16")
    wait(lambda: image(month_region) == current_month)

    click(1014, 70)
    send("move 1186 16")
    wait(lambda: image(month_region) != current_month)
    subprocess.run([args.wtype, "-s", "100", "-k", "Escape"], env=env, check=True, timeout=5)
    wait(lambda: image() == before)
    click(1213, 16)
    wait(lambda: image(month_region) == current_month)
    click(20, 400)
    wait(lambda: image() == before)
    click(1213, 16)
    wait(lambda: image(month_region) == current_month)
    ctl("launcher")
    def dialog_mapped():
        return tomllib.loads(ctl("dialog", "state").stdout)["mapped"]
    wait(dialog_mapped)
    wait(lambda: image(month_region) == before_month)
    subprocess.run([args.wtype, "-s", "100", "-k", "Escape"], env=env, check=True, timeout=5)
    wait(lambda: not dialog_mapped())
    pointer.stdin.write("quit\n")
    pointer.stdin.flush()
    pointer.wait(timeout=3)
    assert opened != before
    result = {"passed": True, "clock_click_opens_calendar": True, "escape_dismisses": True, "month_navigation": True, "today_resets": True, "reopens_on_today": True, "outside_click_dismisses": True, "launcher_dismisses_calendar": True}
    (directory / "calendar.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
