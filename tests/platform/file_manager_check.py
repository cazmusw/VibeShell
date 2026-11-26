"""Exercise the installed GTK file manager in a private directory and compositor."""

import base64
import json
import os
from pathlib import Path
import select
import stat
import struct
import subprocess
import sys
import time
import tomllib
import zipfile
import zlib


def run(args, env, root, children, logs, ctl, wait, windows):
    home = root / "files-home"
    project = home / "project"
    notes = project / "notes"
    notes.mkdir(parents=True)
    (notes / "example.txt").write_text("Mosaic fixture\n")
    (notes / "move.txt").write_text("Move fixture\n")
    desktop_id = "org.mosaic.Files.desktop"
    ctl("pin", desktop_id)
    wait(lambda: desktop_id in tomllib.loads(ctl("config", "get").stdout)["pins"])
    ctl("unpin", desktop_id)
    files_env = dict(env, HOME=str(home))
    desktop_folder = home / "Desktop"
    desktop_folder.mkdir()
    template_dir = home / "Templates"
    template_dir.mkdir()
    template = template_dir / "Letter.txt"
    template.write_bytes(b"Template text\n\x00End\n")
    (Path(files_env["XDG_CONFIG_HOME"]) / "user-dirs.dirs").write_text(
        'XDG_TEMPLATES_DIR="$HOME/Templates"\n'
        'XDG_DESKTOP_DIR="$HOME/Desktop"\n'
    )
    if args.mime_data:
        files_env["XDG_DATA_DIRS"] += ":" + args.mime_data
    launched_file = home / "opened-from-zip"
    opener = root / "archive-opener.py"
    opener.write_text(
        "from pathlib import Path\nimport sys\n"
        "source = Path(sys.argv[1])\n"
        "assert source.name == 'example.txt' and source.parent.name.startswith('.mosaic-archive-')\n"
        f"Path({str(launched_file)!r}).write_text(source.read_text())\n"
    )
    launchers = Path(files_env["XDG_DATA_HOME"]) / "applications"
    launchers.mkdir(parents=True, exist_ok=True)
    (launchers / "archive-opener.desktop").write_text(
        f'[Desktop Entry]\nType=Application\nName=Archive Test Opener\n'
        f'Exec="{sys.executable}" "{opener}" %f\nMimeType=text/plain;\n'
    )
    (Path(files_env["XDG_CONFIG_HOME"]) / "mimeapps.list").write_text(
        "[Default Applications]\ntext/plain=archive-opener.desktop;\n"
    )
    pointer = subprocess.Popen([args.pointer, "1280", "720"], env=env, stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, text=True)
    children.append(pointer)
    log = (root / "mosaic-files.log").open("w+")
    logs.append(log)
    app = subprocess.Popen([args.files, notes.as_uri()], env=files_env, stdout=log, stderr=log)
    children.append(app)

    def active_window():
        return next((window for window in windows() if window[3] == "true"), None)

    def send(*keys):
        subprocess.run([args.wtype, "-s", "100", *keys], env=env, check=True, timeout=5)

    def control(key):
        send("-M", "ctrl", "-s", "100", "-k", key, "-s", "100", "-m", "ctrl")

    def pointer_command(command):
        pointer.stdin.write(command + "\n")
        pointer.stdin.flush()
        assert select.select([pointer.stdout], [], [], 3)[0]
        assert pointer.stdout.readline().strip() == "ok"

    def click(x, y, button=272, settle=0):
        pointer_command(f"move {x} {y}")
        if settle:
            time.sleep(settle)
        pointer_command(f"button {button} 1")
        pointer_command(f"button {button} 0")

    def create_folder(parent, name):
        # Navigation and the toolbar remain asynchronous even for local directories.
        time.sleep(.3)
        click(390, 188)
        time.sleep(.25)
        send("-d", "12", name, "-s", "110", "-k", "Return")
        try:
            wait(lambda: (parent / name).is_dir())
        except TimeoutError as error:
            names = sorted(str(path.relative_to(home)) for path in home.rglob("*"))
            raise AssertionError(f"Failed to create {name}, files: {names}, windows: {windows()}") from error
        wait(lambda: (active := active_window()) and active[:2] == main_window[:2])
        # GIO reports creation on disk before its completion callback starts the refresh.
        time.sleep(.3)

    def open_folder(folder):
        subprocess.run([args.files, folder.as_uri()], env=files_env, check=True, timeout=5)
        time.sleep(.45)

    def preview(name):
        if args.grim:
            subprocess.run([args.grim, "-o", "HEADLESS-1", str(Path(args.report).with_name(name))],
                           env=env, check=True, timeout=5)

    main_window = wait(lambda: (active := active_window()) and active[2] == '"org.mosaic.Files"' and active)
    new_source, new_target, marquee_table, marquee_grid = (
        home / name for name in ("new-source", "new-target", "marquee-table", "marquee-grid")
    )
    for folder in (new_source, new_target, marquee_table, marquee_grid):
        folder.mkdir()

    def name_new(parent, name):
        wait(lambda: (active := active_window()) and active[:2] != main_window[:2])
        send("-d", "12", name, "-s", "100", "-k", "Return")
        wait(lambda: (parent / name).exists())
        time.sleep(.35)

    open_folder(new_source)
    click(684, 414, 273)
    time.sleep(.3)
    preview("preview-files-create-menu.png")
    click(684, 436, settle=.2)
    name_new(new_source, "ready.txt")
    assert (new_source / "ready.txt").read_bytes() == b""
    # The newly created file should already be selected when Copy is clicked.
    click(611, 195)
    open_folder(new_target)
    click(655, 195)
    wait(lambda: (new_target / "ready.txt").is_file())
    time.sleep(.3)
    control("w")
    time.sleep(.3)

    click(684, 414, 273)
    time.sleep(.3)
    click(684, 466, settle=.2)
    name_new(new_source, "a-folder")
    assert (new_source / "a-folder").is_dir()

    open_folder(new_source / "a-folder")
    click(684, 414, 273)
    time.sleep(.3)
    preview("preview-files-create-in-folder.png")
    click(684, 436, settle=.2)
    name_new(new_source / "a-folder", "inside.txt")
    control("w")
    time.sleep(.3)

    def drag(points):
        x, y = points[0]
        pointer_command(f"move {x} {y}")
        time.sleep(.15)
        pointer_command("button 272 1")
        for x, y in points[1:]:
            pointer_command(f"move {x} {y}")
            time.sleep(.07)

    transfer = home / "external-transfer"
    transfer.mkdir()
    transfer_files = [transfer / name for name in ("a with spaces.txt", "b.txt", "я.txt")]
    for file in transfer_files:
        file.write_text(file.name)

    received = root / "external-transfers.jsonl"
    receiver = subprocess.Popen(
        [sys.executable, str(Path(__file__).with_name("file_transfer_receiver.py")), str(received)],
        env=env,
    )
    children.append(receiver)
    receiver_window = wait(lambda: (active := active_window())
                           and active[2] == '"org.mosaic.TransferReceiver"' and active)

    def activate(win):
        ctl("window", "activate", *win[:2])
        wait(lambda: (active := active_window()) and active[:2] == win[:2])
        time.sleep(.2)

    def expect_transfer(kind, value, index):
        events = wait(lambda: received.exists()
                      and len(lines := received.read_text().splitlines()) > index
                      and lines)
        assert json.loads(events[index]) == [kind, value], events[index]

    def drop_on_receiver(x, y):
        drag([(x, y), (x + 45, y + 10), (810, 355), (975, 450), (1020, 478)])
        pointer_command("button 272 0")

    uris = [file.as_uri() for file in transfer_files]
    activate(main_window)
    open_folder(transfer)
    click(420, 264)
    control("a")
    control("c")
    activate(receiver_window)
    control("v")
    expect_transfer("paste", uris, 0)

    activate(main_window)
    drop_on_receiver(420, 264)
    expect_transfer("drag", uris, 1)

    activate(main_window)
    control("2")
    time.sleep(.3)
    click(446, 282)
    drop_on_receiver(598, 282)
    expect_transfer("drag", [uris[1]], 2)

    activate(main_window)
    drag([(446, 282), (491, 292), (810, 355), (1204, 391), (1230, 404)])
    pointer_command("button 272 0")
    time.sleep(.25)
    assert len(received.read_text().splitlines()) == 3

    activate(main_window)
    control("l")
    send("clipboard text")
    control("a")
    control("c")
    activate(receiver_window)
    send("-M", "ctrl", "-M", "shift", "-k", "v", "-m", "shift", "-m", "ctrl")
    expect_transfer("text", "clipboard text", 3)
    assert all(file.read_text() == file.name for file in transfer_files)
    ctl("window", "close", *receiver_window[:2])
    assert receiver.wait(timeout=5) == 0
    activate(main_window)
    send("-k", "Escape")
    control("w")
    time.sleep(.35)

    drag([(624, 420), (597, 395), (535, 337), (470, 287), (392, 248)])
    preview("preview-files-marquee-table.png")
    pointer_command("button 272 0")
    click(611, 195)
    open_folder(marquee_table)
    click(655, 195)
    wait(lambda: (marquee_table / "ready.txt").is_file()
         and (marquee_table / "a-folder/inside.txt").is_file())
    time.sleep(.3)
    control("w")
    time.sleep(.3)
    control("2")
    time.sleep(.3)
    drag([(670, 420), (645, 390), (575, 345), (490, 290), (372, 235)])
    preview("preview-files-marquee-grid.png")
    pointer_command("button 272 0")
    click(611, 195)
    open_folder(marquee_grid)
    click(655, 195)
    wait(lambda: (marquee_grid / "ready.txt").is_file()
         and (marquee_grid / "a-folder/inside.txt").is_file())
    time.sleep(.3)
    control("w")
    time.sleep(.3)
    control("w")

    time.sleep(.5)
    preview("preview-files-breadcrumbs.png")
    click(954, 147)
    create_folder(project, "breadcrumb-target")
    send("-M", "alt", "-k", "Left", "-m", "alt")
    time.sleep(.35)
    control("l")
    send("unsubmitted", "-k", "Escape")
    create_folder(notes, "address-cancelled")
    (notes / "address-cancelled").rmdir()
    (project / "breadcrumb-target").rmdir()
    send("-k", "F5")

    context_source = home / "context-source"
    context_folder = context_source / "a-folder"
    context_folder.mkdir(parents=True)
    (context_source / "one.txt").write_text("Context fixture")
    open_folder(context_source)
    click(806, 264, 273, settle=.2)
    time.sleep(.3)
    preview("preview-files-context.png")
    click(806, 285, settle=.2)
    create_folder(context_folder, "context-opened")
    send("-M", "alt", "-k", "Left", "-m", "alt")
    time.sleep(.4)
    control("2")
    time.sleep(.26)
    click(570, 266, 273, settle=.2)
    time.sleep(.3)
    preview("preview-files-grid-context.png")
    click(570, 506, settle=.2)
    wait(lambda: (active := active_window()) and active[:2] != main_window[:2])
    preview("preview-files-properties.png")
    send("-k", "Escape")
    wait(lambda: (active := active_window()) and active[:2] == main_window[:2])
    control("1")
    time.sleep(.3)
    click(806, 264, 273, settle=.2)
    time.sleep(.3)
    preview("preview-files-copy-path.png")
    click(806, 391, settle=.2)
    control("l")
    control("v")
    send("-k", "Return")
    create_folder(context_folder, "clipboard-path-opened")
    control("w")
    time.sleep(.35)

    view_source, view_a, view_b, view_c = (home / name for name in ("view-source", "view-a", "view-b", "view-c"))
    for folder in (view_source, view_a, view_b, view_c):
        folder.mkdir()
    for name, data in (("z-tiny.txt", "x"), ("middle.txt", "12345"), ("a-large.txt", "1234567890")):
        (view_source / name).write_text(data)

    open_folder(view_source)
    click(1090, 228)
    time.sleep(.25)
    click(420, 264)
    control("2")
    time.sleep(.3)
    if args.grim:
        subprocess.run([args.grim, "-o", "HEADLESS-1", str(Path(args.report).with_name("preview-files-grid.png"))],
                       env=env, check=True, timeout=5)
    click(611, 188)
    open_folder(view_a)
    control("1")
    time.sleep(.22)
    click(655, 188)
    wait(lambda: (view_a / "z-tiny.txt").is_file())
    time.sleep(.35)
    control("w")
    time.sleep(.3)
    click(570, 265)
    click(611, 188)
    open_folder(view_b)
    click(655, 188)
    try:
        wait(lambda: (view_b / "middle.txt").is_file())
    except TimeoutError as error:
        raise AssertionError(f"Second tile should contain middle.txt, copied {list(view_b.iterdir())}") from error
    time.sleep(.35)
    control("w")
    time.sleep(.3)
    control("1")
    time.sleep(.3)
    click(420, 366)
    click(611, 188)
    open_folder(view_c)
    click(655, 188)
    wait(lambda: (view_c / "a-large.txt").is_file())
    time.sleep(.35)
    control("w")
    time.sleep(.3)
    nested = view_source / "nested"
    nested.mkdir()
    send("-k", "F5")
    control("2")
    time.sleep(.35)
    click(420, 266)
    send("-k", "Return")
    create_folder(nested, "opened-from-grid")
    control("w")

    # Let the initial asynchronous URI lookup populate the toolbar.
    time.sleep(.4)
    click(1090, 228)  # The smaller file now precedes example.txt.
    time.sleep(.25)
    if args.grim:
        subprocess.run([args.grim, "-o", "HEADLESS-1", str(Path(args.report).with_name("preview-files-sort.png"))],
                       env=env, check=True, timeout=5)
    for y, name, moved in [
        (264, "move.txt", True),
        (264, "example.txt", False),
    ]:
        click(420, y)
        if moved:
            click(567, 188)
        else:
            control("c")
        send("-M", "ctrl", "-k", "t", "-m", "ctrl",
             "-s", "220", "-M", "alt", "-k", "Up", "-m", "alt")
        time.sleep(.4)
        if moved:
            click(655, 188)
        else:
            click(420, 264)
            control("v")
        try:
            wait(lambda: (project / name).is_file())
        except TimeoutError as error:
            names = sorted(str(path.relative_to(home)) for path in home.rglob("*"))
            raise AssertionError(f"Failed to transfer {name}, files: {names}") from error
        assert (project / name).read_text() == ("Move fixture\n" if moved else "Mosaic fixture\n")
        assert (notes / name).exists() != moved
        time.sleep(.3)
        if not moved:
            click(655, 188)
            wait(lambda: (active := active_window()) and active[:2] != main_window[:2])
            send("-k", "Escape")
            wait(lambda: (active := active_window()) and active[:2] == main_window[:2])
            assert set(project.glob("example*")) == {project / name}
            time.sleep(.3)
            click(655, 188)
            wait(lambda: (active := active_window()) and active[:2] != main_window[:2])
            send("-k", "Tab", "-s", "120", "-k", "Return")
            duplicate = project / "example — копия.txt"
            wait(duplicate.is_file)
            assert (project / name).read_text() == duplicate.read_text() == "Mosaic fixture\n"
            time.sleep(.3)
        send("-M", "ctrl", "-k", "w", "-m", "ctrl")
        time.sleep(.3)

    create_folder(notes, "uri-opened")
    send("-M", "ctrl", "-k", "t", "-m", "ctrl",
         "-s", "180", "-M", "alt", "-k", "Up", "-m", "alt")
    create_folder(project, "parent-opened")
    send("-M", "alt", "-k", "Left", "-m", "alt")
    create_folder(notes, "back-opened")
    send("-M", "alt", "-k", "Right", "-m", "alt")
    create_folder(project, "forward-opened")
    control("l")
    send("-d", "12", str(home), "-s", "100", "-k", "Return")
    create_folder(home, "address-opened")
    send("-M", "ctrl", "-k", "w", "-m", "ctrl")
    create_folder(notes, "original-tab")

    if args.grim:
        time.sleep(.25)
        subprocess.run([args.grim, "-o", "HEADLESS-1", str(Path(args.report).with_name("preview-files.png"))],
                       env=env, check=True, timeout=5)

    source, copies, destination = (home / name for name in ("group-source", "group-copy", "group-move"))
    for folder in (source, copies, destination):
        folder.mkdir()
    first, conflict, last = ("a-first.txt", "b-conflict.txt", "z-last.txt")
    for name in (first, conflict, last):
        (source / name).write_text(name)
    (destination / conflict).write_text("existing")

    def modified_click(modifier, y):
        hold = subprocess.Popen(
            [args.wtype, "-M", modifier, "-s", "650", "-m", modifier],
            env=env,
        )
        children.append(hold)
        time.sleep(.26)
        click(420, y)
        assert hold.wait(timeout=4) == 0

    open_folder(source)
    click(420, 264)
    modified_click("shift", 366)
    if args.grim:
        subprocess.run([args.grim, "-o", "HEADLESS-1", str(Path(args.report).with_name("preview-files-group.png"))],
                       env=env, check=True, timeout=5)
    modified_click("ctrl", 315)
    click(420, 264, 273)
    time.sleep(.3)
    click(420, 361, settle=.2)
    open_folder(copies)
    click(655, 188)
    wait(lambda: (copies / first).is_file() and (copies / last).is_file())
    assert not (copies / conflict).exists()

    # The copies are test files in the private home and data directories.
    time.sleep(.33)
    click(420, 264)
    control("a")
    click(479, 188)
    wait(lambda: (active := active_window()) and active[:2] != main_window[:2])
    send("-k", "Escape")
    wait(lambda: (active := active_window()) and active[:2] == main_window[:2])
    assert all((copies / name).exists() for name in (first, last))
    click(479, 188)
    wait(lambda: (active := active_window()) and active[:2] != main_window[:2])
    send("-k", "Tab", "-s", "120", "-k", "Return")
    wait(lambda: not any((copies / name).exists() for name in (first, last)))

    open_folder(source)
    click(420, 264)
    control("a")
    control("x")
    open_folder(destination)
    click(655, 188)
    wait(lambda: (destination / first).is_file())
    wait(lambda: (active := active_window()) and active[:2] != main_window[:2])
    send("-k", "Escape")
    wait(lambda: (active := active_window()) and active[:2] == main_window[:2])
    assert (destination / conflict).read_text() == "existing"
    assert not (source / first).exists()
    assert all((source / name).is_file() for name in (conflict, last))
    (source / first).write_text("recreated after the move")
    (destination / conflict).unlink()
    time.sleep(.35)
    click(655, 188)
    wait(lambda: all((destination / name).is_file() for name in (first, conflict, last)))
    assert list(source.iterdir()) == [source / first]
    assert (source / first).read_text() == "recreated after the move"
    assert (destination / first).read_text() == first

    bookmarks = Path(files_env["XDG_CONFIG_HOME"]) / "mosaic-files/bookmarks.toml"
    open_folder(notes)
    # The first built-in place below Home previously linked to XDG Desktop.
    click(220, 181, settle=.2)
    time.sleep(.35)
    control("d")
    wait(lambda: bookmarks.exists() and tomllib.loads(bookmarks.read_text())["folders"])
    assert str(desktop_folder) not in tomllib.loads(bookmarks.read_text())["folders"]
    control("d")
    wait(lambda: not tomllib.loads(bookmarks.read_text())["folders"])

    open_folder(notes)
    control("d")
    wait(lambda: bookmarks.exists() and str(notes) in tomllib.loads(bookmarks.read_text())["folders"])
    preview("preview-files-bookmarks.png")

    denied = home / "protected-fixture"
    denied.mkdir()
    denied.chmod(0)
    try:
        if not os.access(denied, os.R_OK | os.X_OK):
            open_folder(denied)
            wait(lambda: (active := active_window()) and active[:2] != main_window[:2])
            preview("preview-files-access-denied.png")
            send("-k", "Escape")
            wait(lambda: (active := active_window()) and active[:2] == main_window[:2])
    finally:
        denied.chmod(0o700)

    sensitive = "private-path-test-fixture"
    subprocess.run([args.files, str(notes / sensitive)], env=files_env, check=True, timeout=5)
    wait(lambda: '\tfiles\topen_directory_failed\t' in ctl("logs").stdout)
    assert sensitive not in ctl("logs").stdout
    if args.grim:
        def sidebar_sample():
            return subprocess.check_output([args.grim, "-g", "293,543 53x18", "-"],
                                           env=env, timeout=5)

        before_narrowing = sidebar_sample()
        drag([(360, 402), (326, 402), (298, 402), (278, 402)])
        pointer_command("button 272 0")
        wait(lambda: sidebar_sample() != before_narrowing, seconds=4)
        preview("preview-files-narrow-sidebar.png")
        drag([(278, 402), (298, 402), (326, 402), (360, 402)])
        pointer_command("button 272 0")
        wait(lambda: sidebar_sample() == before_narrowing, seconds=4)
    photos = home / "photos"
    photos.mkdir()
    photo = photos / "a.png"

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    def write_photo(path, width, height, rows):
        header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
        path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
                         + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))

    write_photo(photo, 1000, 600, (b"\x00" + b"\xff\x00\x00" * 1000) * 600)
    # Solid green and blue 1×1 fixtures encoded as JPEG and lossless WebP.
    (photos / "b.JPEG").write_bytes(base64.b64decode(
        "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIs"
        "IxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/2wBDAQkJCQwLDBgNDRgyIRwhMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIy"
        "MjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjL/wAARCAABAAEDASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAA"
        "AAX/xAAUEAEAAAAAAAAAAAAAAAAAAAAA/8QAFAEBAAAAAAAAAAAAAAAAAAAAB//EABQRAQAAAAAAAAAAAAAAAAAA"
        "AAD/2gAMAwEAAhEDEQA/AKwA0ED/2Q=="
    ))
    (photos / "c.webp").write_bytes(base64.b64decode("UklGRhwAAABXRUJQVlA4TA8AAAAvAAAAAAcQ0f/+ByKi/wEA"))
    (photos / "d-corrupt.png").write_bytes(b"invalid image")
    write_photo(photos / "e-oversize.png", 12000, 12000, b"")

    def photo_window():
        return wait(lambda: (active := active_window()) and active[:2] != main_window[:2]
                    and active[2] == '"org.mosaic.Files"', seconds=4)

    subprocess.run([args.files, photo.as_uri()], env=files_env, check=True, timeout=5)
    photo_window()
    send("-k", "Escape")
    wait(lambda: (active := active_window()) and active[:2] == main_window[:2])
    open_folder(photos)
    click(806, 264, 273, settle=.2)
    time.sleep(.3)
    click(806, 285, settle=.2)
    photo_window()

    if args.grim:
        def photo_pixel(x, y):
            return subprocess.check_output([args.grim, "-t", "ppm", "-g", f"{x},{y} 1x1", "-"],
                                           env=env, timeout=5)[-3:]

        red = b"\xff\x00\x00"
        wait(lambda: photo_pixel(640, 360) == red and photo_pixel(345, 360) == red)
        preview("preview-files-photo.png")
        send("-k", "minus")
        wait(lambda: photo_pixel(640, 360) == red and photo_pixel(345, 360) != red)
        send("-k", "0")
        wait(lambda: photo_pixel(345, 360) == red)
        send("-k", "Right")
        wait(lambda: (rgb := photo_pixel(640, 360))[1] > 240 and rgb[0] < 10 and rgb[2] < 10)
        send("-k", "Right")
        wait(lambda: photo_pixel(640, 360) == b"\x00\x00\xff")
    else:
        send("-k", "Right", "-k", "Right")
    send("-k", "Right")
    wait(lambda: '\tfiles\tphoto_decode_failed\t' in ctl("logs").stdout)
    preview("preview-files-photo-corrupt.png")
    send("-k", "Right")
    wait(lambda: '\tfiles\tphoto_limit_exceeded\t' in ctl("logs").stdout)
    send("-k", "Left", "-k", "Left")
    if args.grim:
        wait(lambda: photo_pixel(640, 360) == b"\x00\x00\xff")
    send("-k", "Escape")
    wait(lambda: (active := active_window()) and active[:2] == main_window[:2])
    click(806, 264, settle=.2)
    send("-k", "Return")
    photo_window()
    send("-k", "Escape")
    wait(lambda: (active := active_window()) and active[:2] == main_window[:2])
    sample_zip = home / "sample.zip"
    with zipfile.ZipFile(sample_zip, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("notes/example.txt", "Archive fixture\n")
    subprocess.run([args.files, sample_zip.as_uri()], env=files_env, check=True, timeout=5)
    def files_modal(previous):
        return wait(lambda: (active := active_window()) and active[2] == '"org.mosaic.Files"'
                    and active[:2] != previous[:2] and active, seconds=4)

    files_modal(main_window)
    # The busy dialog for loading the index can briefly take focus first.
    time.sleep(.35)
    archive_window = files_modal(main_window)
    preview("preview-files-archive.png")
    click(433, 125, settle=.2)
    wait(lambda: launched_file.is_file() and launched_file.read_text() == "Archive fixture\n")
    wait(lambda: (active := active_window()) and active[:2] == archive_window[:2])
    send("-M", "ctrl", "-k", "e", "-m", "ctrl")
    files_modal(archive_window)
    send("-k", "Escape")
    wait(lambda: (active := active_window()) and active[:2] == archive_window[:2])
    assert not (home / "sample").exists()
    send("-M", "ctrl", "-k", "e", "-m", "ctrl")
    files_modal(archive_window)
    send("-k", "Return")
    unpacked = home / "sample/notes/example.txt"
    wait(unpacked.is_file)
    assert unpacked.read_text() == "Archive fixture\n"
    wait(lambda: (active := active_window()) and active[:2] == archive_window[:2])
    send("-k", "Escape")
    wait(lambda: (active := active_window()) and active[:2] == main_window[:2])

    zip_source = home / "zip-source"
    zip_source.mkdir()
    (zip_source / "message.txt").write_text("Packed from the context menu\n")
    open_folder(zip_source)
    click(806, 264, 273, settle=.2)
    time.sleep(.3)
    preview("preview-files-archive-actions.png")
    click(806, 545, settle=.2)
    files_modal(main_window)
    send("-k", "Return")
    packed = zip_source / "message.txt.zip"
    wait(packed.is_file)
    with zipfile.ZipFile(packed) as archive:
        assert archive.read("message.txt") == b"Packed from the context menu\n"
    wait(lambda: (active := active_window()) and active[:2] == main_window[:2])
    (zip_source / "message.txt").rename(zip_source / "z-original.txt")
    send("-k", "F5")
    time.sleep(.35)
    click(806, 264, 273, settle=.2)
    time.sleep(.3)
    click(806, 285, settle=.2)
    files_modal(main_window)
    time.sleep(.35)
    files_modal(main_window)
    send("-k", "Escape")
    wait(lambda: (active := active_window()) and active[:2] == main_window[:2])
    click(806, 264, 273, settle=.2)
    time.sleep(.3)
    click(806, 575, settle=.2)
    files_modal(main_window)
    time.sleep(.35)
    destination = home / "custom-unpack"
    destination.mkdir()
    control("a")
    send(str(destination), "-s", "100", "-k", "Return")
    copied_entry = destination / "message.txt/message.txt"
    wait(copied_entry.is_file)
    assert copied_entry.read_text() == "Packed from the context menu\n"
    wait(lambda: (active := active_window()) and active[:2] == main_window[:2])

    def open_file_properties(directory):
        open_folder(directory)
        click(806, 264, 273, settle=.2)
        time.sleep(.3)
        click(806, 506, settle=.2)
        files_modal(main_window)
        time.sleep(.3)

    def close_file_properties():
        send("-k", "Escape")
        wait(lambda: (active := active_window()) and active[:2] == main_window[:2])

    permissions = home / "permissions"
    permissions.mkdir()
    editable = permissions / "owned.txt"
    editable.write_text("Permission fixture\n")
    editable.chmod(0o640)
    open_file_properties(permissions)
    send("-k", "space")
    control("s")
    wait(lambda: stat.S_IMODE(editable.stat().st_mode) == 0o240, seconds=4)
    time.sleep(.25)
    preview("preview-files-permissions.png")
    close_file_properties()

    folders = permissions / "folders"
    folder = folders / "child"
    folder.mkdir(parents=True)
    folder.chmod(0o750)
    open_file_properties(folders)
    send("-k", "Tab", "-k", "Tab", "-k", "space")
    control("s")
    wait(lambda: stat.S_IMODE(folder.stat().st_mode) == 0o650, seconds=4)
    close_file_properties()
    folder.chmod(0o750)

    links = permissions / "links"
    links.mkdir()
    (links / "link.txt").symlink_to(editable)
    open_file_properties(links)
    control("s")
    assert stat.S_IMODE(editable.stat().st_mode) == 0o240
    preview("preview-files-permissions-link.png")
    close_file_properties()

    readonly = Path(args.shell).resolve().with_name("mosaic-alt-tab")
    readonly_stat = readonly.stat()
    nonowner = os.geteuid() not in (0, readonly_stat.st_uid) and readonly.parts[:3] == ("/", "nix", "store")
    if nonowner:
        open_file_properties(readonly.parent)
        send("-k", "space")
        control("s")
        wait(lambda: '\tfiles\tpermissions_failed\t' in ctl("logs").stdout)
        assert readonly.stat().st_mode == readonly_stat.st_mode
        preview("preview-files-permissions-denied.png")
        close_file_properties()

    from_template = home / "from-template"
    from_template.mkdir()
    open_folder(from_template)

    def choose_template():
        click(684, 414, 273)
        time.sleep(.3)
        click(684, 436, settle=.2)
        files_modal(main_window)
        time.sleep(.35)
        send("-M", "shift", "-k", "Tab", "-m", "shift", "-k", "space", "-k", "Down", "-k", "Return")
        send("-k", "Tab")

    def create_instance(name):
        control("a")
        send(name, "-s", "100", "-k", "Return")
        wait(lambda: (active := active_window()) and active[:2] == main_window[:2])

    choose_template()
    preview("preview-files-template.png")
    create_instance("copy.txt")
    instance = from_template / "copy.txt"
    wait(instance.is_file, seconds=4)
    assert instance.read_bytes() == template.read_bytes()

    choose_template()
    send("-k", "Escape")
    wait(lambda: (active := active_window()) and active[:2] == main_window[:2])
    assert list(from_template.iterdir()) == [instance]

    instance.write_bytes(b"Keep existing content")
    error_event = '\tfiles\tcreate_from_template_failed\t'
    error_count = ctl("logs").stdout.count(error_event)
    choose_template()
    create_instance(instance.name)
    wait(lambda: ctl("logs").stdout.count(error_event) > error_count)
    assert instance.read_bytes() == b"Keep existing content"

    error_count = ctl("logs").stdout.count(error_event)
    choose_template()
    template.unlink()
    create_instance("missing-template.txt")
    wait(lambda: ctl("logs").stdout.count(error_event) > error_count)
    assert list(from_template.iterdir()) == [instance]
    preview("preview-files-template-unavailable.png")

    size_fixture = home / "folder-count-fixture"
    measured = size_fixture / "counted"
    deep = measured / "nested" / "deeper"
    deep.mkdir(parents=True)
    (measured / "top.txt").write_bytes(b"x" * 100)
    counted_file = deep / "nested.txt"
    counted_file.write_bytes(b"y" * 221)
    (measured / "outside").symlink_to(notes, target_is_directory=True)
    size_cache = Path(files_env["XDG_CACHE_HOME"]) / "mosaic-files/folder-sizes.json"

    def cached_folder_bytes():
        if not size_cache.exists():
            return None
        for record in json.loads(size_cache.read_text())["entries"]:
            if bytes(record["path"]) == os.fsencode(measured):
                return record["bytes"]
        return None

    open_folder(size_fixture)
    wait(lambda: cached_folder_bytes() == 321, seconds=5)
    if args.grim:
        def size_cell():
            return subprocess.check_output([args.grim, "-g", "1045,248 94x46", "-"],
                                           env=env, timeout=5)

        before_size_change = size_cell()
        preview("preview-files-folder-sizes.png")

    counted_file.write_bytes(b"z" * 4100)
    send("-k", "F5")
    wait(lambda: cached_folder_bytes() == 4200, seconds=5)
    if args.grim:
        wait(lambda: size_cell() != before_size_change, seconds=5)

    shortcuts = home / "shortcuts"
    shortcuts.mkdir()
    open_folder(shortcuts)
    (shortcuts / "original.txt").write_text("Keep the extension\n")
    click(684, 414, 273)
    time.sleep(.3)
    preview("preview-files-refresh-context.png")
    click(684, 537, settle=.2)
    time.sleep(.45)
    click(420, 264)
    send("-k", "F2")
    files_modal(main_window)
    send("renamed", "-s", "100", "-k", "Return")
    renamed = shortcuts / "renamed.txt"
    wait(renamed.is_file, seconds=4)
    time.sleep(.35)
    click(420, 264)
    control("x")
    click(210, 143)
    time.sleep(.4)
    control("v")
    wait((home / renamed.name).is_file, seconds=4)
    assert not renamed.exists()
    assert (home / renamed.name).read_text() == "Keep the extension\n"

    open_folder(desktop_folder)
    control("d")
    wait(lambda: tomllib.loads(bookmarks.read_text())["folders"] == [str(notes), str(desktop_folder)])
    ctl("window", "close", *main_window[:2])
    assert app.wait(timeout=5) == 0
    app = subprocess.Popen([args.files, notes.as_uri()], env=files_env, stdout=log, stderr=log)
    children.append(app)
    restored_window = wait(lambda: (active := active_window()) and active[2] == '"org.mosaic.Files"' and active)
    time.sleep(.4)
    control("d")
    wait(lambda: tomllib.loads(bookmarks.read_text())["folders"] == [str(desktop_folder)])
    click(224, 207, settle=.2)
    time.sleep(.4)
    control("d")
    wait(lambda: not tomllib.loads(bookmarks.read_text())["folders"])
    ctl("window", "close", *restored_window[:2])
    assert app.wait(timeout=5) == 0

    tab_folders = [home / name for name in ("Aa", "Bb", "Cc", "Dd", "Ee")]
    for folder in tab_folders:
        folder.mkdir()
    app = subprocess.Popen([args.files, *(folder.as_uri() for folder in tab_folders)],
                           env=files_env, stdout=log, stderr=log)
    children.append(app)
    main_window = wait(lambda: (active := active_window()) and active[2] == '"org.mosaic.Files"' and active)
    time.sleep(.5)

    def tab_action(x, row):
        click(x, 98, 273, settle=.2)
        time.sleep(.25)
        click(x, row, settle=.2)
        time.sleep(.25)

    # Refresh Bb while Ee is active, then inspect the newly enumerated file.
    external = tab_folders[1] / "external.txt"
    external.write_text("New tab content\n")
    tab_action(544, 319)
    click(544, 98)
    time.sleep(.35)
    click(420, 264)
    send("-k", "F2")
    files_modal(main_window)
    send("-k", "Escape")
    wait(lambda: (active := active_window()) and active[:2] == main_window[:2])
    click(934, 98)
    time.sleep(.25)

    # Bb moves from the second position to the third, so closing to its right
    # must preserve Aa and Cc, even when the rightmost tab was active.
    click(505, 98, 273, settle=.2)
    time.sleep(.25)
    preview("preview-files-tab-menu.png")
    click(505, 153, settle=.2)
    time.sleep(.25)
    tab_action(674, 287)
    click(804, 98)  # The former fourth slot is now empty.
    create_folder(tab_folders[1], "after-close-right")
    tab_action(544, 257)
    click(414, 98)  # Cc is now the first tab.
    create_folder(tab_folders[2], "after-close-left")
    tab_action(544, 121)
    tab_action(414, 225)
    create_folder(tab_folders[1], "after-close-others")
    tab_action(414, 193)
    assert app.wait(timeout=5) == 0

    pointer.stdin.write("quit\n")
    pointer.stdin.flush()
    assert pointer.wait(timeout=5) == 0
    log.flush()
    log.seek(0)
    stderr = log.read()
    assert not any(marker in stderr for marker in ("Gtk-CRITICAL", "GLib-CRITICAL", "panicked at")), stderr
    result = {"passed": True, "uri_open": True, "new_folder": True,
              "parent_back_forward_address": True, "independent_tabs": True, "dock_registration": True,
              "copy_and_move_between_tabs": True, "conflict_cancel_and_keep_both": True,
              "files_pasted_and_dropped_in_external_window": True, "text_copy_preserved": True,
              "sorted_selection_matches_file": True, "private_directory_error_in_journal": True,
              "shift_range_ctrl_toggle_and_group_copy": True, "group_trash_confirmed": True,
              "partial_move_resumes_without_repeating": True,
              "grid_keeps_selection_and_sort": True, "view_mode_is_per_tab": True,
              "folder_activation_in_grid": True, "breadcrumb_parent_and_cancelled_edit": True,
              "table_context_opens_folder": True, "grid_context_file_properties": True,
              "context_path_clipboard": True, "context_keeps_selected_group": True,
              "empty_folder_context_creation_and_selection": True, "create_inside_nested_folder": True,
              "table_and_grid_marquee_select_mixed_entries": True,
              "bookmarks_survive_restart_and_toggle_off": True,
              "desktop_absent_by_default_and_manual_bookmark_works": True,
              "sidebar_narrows_and_widens": bool(args.grim),
              "built_in_photo_viewer_and_safe_decode_limits": True,
              "photo_formats_navigation_and_zoom_visible": bool(args.grim),
              "archive_viewer_and_extraction": True,
              "archive_member_launch_and_context_actions": True,
              "permissions_edited_from_properties": True,
              "permissions_nonowner_error": nonowner,
              "created_file_from_xdg_template": True,
              "template_cancel_conflict_and_unavailable": True,
              "rename_keeps_extension_and_file_shortcuts_work_after_sidebar_navigation": True,
              "context_refresh_reads_external_changes": True,
              "tab_context_moves_and_closes_relative_to_selected_tab": True,
              "tab_context_refresh_reads_its_own_directory": True,
              "folder_sizes_persist_skip_symlinks_and_refresh_nested_changes": True,
              "access_denied_offer_cancelled": not os.geteuid() == 0}
    Path(args.report).with_name("files.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
