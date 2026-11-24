"""Receive file offers in a separate GTK process."""

import json
from pathlib import Path
import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, GLib, Gtk


def activate(app, report):
    window = Gtk.ApplicationWindow(application=app, title="File transfer receiver",
                                   default_width=240, default_height=180)
    window.set_child(Gtk.Label(label="Drop files or press Ctrl+V"))

    def record(kind, value):
        with report.open("a") as output:
            output.write(json.dumps([kind, value]) + "\n")

    def record_files(kind, files):
        record(kind, [file.get_uri() for file in files.get_files()])

    def dropped(_target, files, _x, _y):
        record_files("drag", files)
        return True

    target = Gtk.DropTarget.new(Gdk.FileList.__gtype__, Gdk.DragAction.COPY)
    target.connect("drop", dropped)
    window.add_controller(target)

    def receive_files(clipboard, result):
        try:
            record_files("paste", clipboard.read_value_finish(result))
        except GLib.Error as error:
            record("paste-error", error.message)

    def receive_text(clipboard, result):
        try:
            record("text", clipboard.read_text_finish(result))
        except GLib.Error as error:
            record("paste-error", error.message)

    def paste(_keys, keyval, _keycode, state):
        if keyval not in (Gdk.KEY_v, Gdk.KEY_V) or not state & Gdk.ModifierType.CONTROL_MASK:
            return False
        clipboard = window.get_clipboard()
        if state & Gdk.ModifierType.SHIFT_MASK:
            clipboard.read_text_async(None, receive_text)
        elif not clipboard.get_formats().contain_mime_type("text/uri-list"):
            record("paste-error", "Clipboard does not offer text/uri-list")
        else:
            clipboard.read_value_async(Gdk.FileList.__gtype__, GLib.PRIORITY_DEFAULT, None,
                                       receive_files)
        return True

    keys = Gtk.EventControllerKey.new()
    keys.connect("key-pressed", paste)
    window.add_controller(keys)
    window.present()


app = Gtk.Application(application_id="org.mosaic.TransferReceiver")
app.connect("activate", activate, Path(sys.argv[1]))
raise SystemExit(app.run(sys.argv[:1]))
