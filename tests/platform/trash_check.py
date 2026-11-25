"""Exercise Dock trash controls on a private Labwc/GVfs session."""
import json
import os
from pathlib import Path
import pwd
import select
import subprocess
import time
import tomllib


def run(args, env, children, ctl, wait, root, uri_marker):
    if args.confirm_trash_delete:
        # GVfs may aggregate trash from mounted volumes. Run destructive
        # interaction only as a dedicated Nix build user.
        assert os.environ.get("NIX_BUILD_TOP")
        assert pwd.getpwuid(os.getuid()).pw_name.startswith("nixbld")

    def status():
        return ctl("trash", "status").stdout.strip()

    def dialog():
        return tomllib.loads(ctl("dialog", "state").stdout)

    wait(lambda: status() == "empty")
    sample = root / "test-file"
    sample.write_text("only a fixture")
    folder = root / "test-directory"
    folder.mkdir()
    canary = root / "outside-trash"
    canary.write_text("keep")
    (folder / "nested").write_text("nested fixture")
    (folder / "link").symlink_to(canary)
    subprocess.run([args.gio, "trash", str(sample), str(folder)], env=env, check=True, timeout=5)
    wait(lambda: status() == "full")
    def inventory():
        return set(subprocess.check_output([args.gio, "list", "trash:///"], env=env, timeout=5)
                   .decode().splitlines())
    expected = {sample.name, folder.name}
    assert inventory() == expected

    pointer = subprocess.Popen([args.pointer, "1280", "720"], env=env, stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, text=True)
    children.append(pointer)
    def send(command):
        pointer.stdin.write(command + "\n")
        pointer.stdin.flush()
        assert select.select([pointer.stdout], [], [], 3)[0]
        assert pointer.stdout.readline().strip() == "ok"
    def click(button):
        send(f"button {button} 1")
        send(f"button {button} 0")
    def keys(*names):
        command = [args.wtype, "-s", "100"]
        for name in names:
            command.extend(["-k", name])
        subprocess.run(command, env=env, check=True, timeout=5)

    dock = tomllib.loads(ctl("dock", "state").stdout)
    send(f"move {640+dock['width']//2-88} {720-12-dock['height']//2}")
    time.sleep(.15)
    click(273)
    wait(lambda: dialog()["mapped"] and dialog()["kind"] == "empty-trash")
    click(272)
    wait(uri_marker.exists)
    assert uri_marker.read_text() == "trash:///"
    assert not dialog()["mapped"]
    click(273)
    wait(lambda: dialog()["mapped"] and dialog()["kind"] == "empty-trash")
    keys("Escape")
    wait(lambda: not dialog()["mapped"])
    assert status() == "full"
    if args.confirm_trash_delete:
        assert inventory() == expected
        click(273)
        wait(lambda: dialog()["mapped"] and dialog()["kind"] == "empty-trash")
        keys("Tab", "Return")
        wait(lambda: status() == "empty")
        assert inventory() == set()
        assert canary.read_text() == "keep"

    pointer.stdin.write("quit\n")
    pointer.stdin.flush()
    assert pointer.wait(timeout=3) == 0
    summary = {"passed": True, "empty_to_full_via_monitor": True, "opened_through_uri_handler": True,
               "escape_cancelled_cleanup": True, "confirmed_cleanup": bool(args.confirm_trash_delete)}
    (Path(args.report).parent / "trash.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary), flush=True)


def files_trash(args, env, children, logs, ctl, wait, root, windows):
    # The GVfs trash can include data from mounted volumes.
    assert args.confirm_trash_delete and os.environ.get("NIX_BUILD_TOP")
    assert pwd.getpwuid(os.getuid()).pw_name.startswith("nixbld")

    def inventory():
        return set(subprocess.check_output([args.gio, "list", "trash:///"], env=env, timeout=5)
                   .decode().splitlines())

    assert not inventory()
    sample = root / "files-trash-item"
    sample.write_text("fixture for Mosaic Files")
    folder = root / "files-trash-folder"
    folder.mkdir()
    outside = root / "files-trash-canary"
    outside.write_text("keep")
    (folder / "child").write_text("nested fixture")
    (folder / "link").symlink_to(outside)
    subprocess.run([args.gio, "trash", str(sample), str(folder)], env=env, check=True, timeout=5)
    expected = {sample.name, folder.name}
    assert inventory() == expected

    log = (root / "files-trash.log").open("w+")
    logs.append(log)
    app = subprocess.Popen([args.files_trash, 'trash:///'], env=env, stdout=log, stderr=log)
    children.append(app)
    def files_window():
        return next((row for row in windows() if 'org.mosaic.Files' in row[2]), None)
    wait(files_window)
    original_windows = {tuple(row[:2]) for row in windows()}

    pointer = subprocess.Popen([args.pointer, "1280", "720"], env=env, stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, text=True)
    children.append(pointer)
    def click(x, y):
        for command in (f"move {x} {y}", "button 272 1", "button 272 0"):
            pointer.stdin.write(command + "\n")
            pointer.stdin.flush()
            assert select.select([pointer.stdout], [], [], 3)[0]
            assert pointer.stdout.readline().strip() == "ok"

    def key(name):
        subprocess.run([args.wtype, "-s", "100", "-k", name], env=env, check=True, timeout=5)

    def confirmation():
        return any(tuple(row[:2]) not in original_windows for row in windows())

    def open_confirmation():
        def try_open():
            if confirmation():
                return True
            # The compositor can report the dialog gone before the parent
            # receives focus and the async callback restores sensitivity.
            if (window := files_window()) and window[3] == "true":
                click(1050, 151)
            return False
        wait(try_open, seconds=4)

    def preview(name):
        subprocess.run([args.grim, '-o', 'HEADLESS-1', str(Path(args.report).parent / name)],
                       env=env, check=True, timeout=5)

    def listing():
        return subprocess.check_output([args.grim, '-g', '375,250 180x96', '-'], env=env, timeout=5)

    time.sleep(.4)
    preview('preview-files-trash.png')
    before = listing()
    open_confirmation()
    preview('preview-files-trash-confirm.png')
    key("Escape")
    wait(lambda: not confirmation())
    assert inventory() == expected
    open_confirmation()
    key("Return")
    wait(lambda: not confirmation())
    assert inventory() == expected
    open_confirmation()
    click(750, 356)
    wait(lambda: not inventory())
    wait(lambda: not confirmation())
    wait(lambda: listing() != before)
    assert outside.read_text() == "keep"
    time.sleep(.2)
    click(1050, 151)
    time.sleep(.2)
    assert not confirmation()
    result = {"passed": True, "cancel_keeps_every_item": True, "confirmed_cleanup": True,
              "default_response_cancels": True, "empty_action_disabled": True,
              "listing_refreshed": True, "symlink_target_preserved": True}
    (Path(args.report).parent / "files-trash.json").write_text(json.dumps(result) + "\n")
    print(json.dumps(result), flush=True)
