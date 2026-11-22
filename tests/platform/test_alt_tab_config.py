import importlib.machinery
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
import xml.etree.ElementTree as ET


script = Path(__file__).resolve().parents[2] / "scripts/mosaic-alt-tab"
loader = importlib.machinery.SourceFileLoader("mosaic_alt_tab", os.fspath(script))
spec = importlib.util.spec_from_loader(loader.name, loader)
alt_tab = importlib.util.module_from_spec(spec)
loader.exec_module(alt_tab)


class AltTabConfigTest(unittest.TestCase):
    def test_loads_thumbnail_grid_preserving_existing_actions(self):
        before = b"""<labwc_config><theme><name>Example</name></theme>
        <!-- preserve this comment -->
        <keyboard>
          <default/>
          <keybind key="W-q"><action name="Close"/></keybind>
          <keybind key="Mod1-Tab"><action name="NextWindowImmediate"/></keybind>
          <keybind key="S-A-Tab"><action name="PreviousWindowImmediate"/></keybind>
        </keyboard>
        <windowSwitcher><osd show="no" output="all"/></windowSwitcher>
        </labwc_config>"""
        data = alt_tab.configure(before)
        root = ET.fromstring(data)
        self.assertEqual(root.findtext("theme/name"), "Example")
        self.assertIn(b"preserve this comment", data)
        self.assertIsNotNone(root.find("keyboard/default"))
        self.assertEqual(
            [(bind.attrib["key"], bind.find("action").attrib["name"])
             for bind in root.findall("keyboard/keybind")],
            [("W-q", "Close"), ("A-Tab", "NextWindow"), ("A-S-Tab", "PreviousWindow")],
        )
        self.assertEqual(root.find("windowSwitcher/osd").attrib["style"], "thumbnail")
        self.assertEqual(alt_tab.configure(data), data)

    def test_missing_keyboard_preserves_default_shortcuts(self):
        root = ET.fromstring(alt_tab.configure(b"<labwc_config/>"))
        self.assertIsNotNone(root.find("keyboard/default"))

    def test_apply_leaves_backup_and_refuses_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "rc.xml"
            original = b"<labwc_config/>"
            config.write_bytes(original)
            backup = alt_tab.install(config, original, alt_tab.configure(original))
            self.assertEqual(backup.read_bytes(), original)
            self.assertEqual(ET.parse(config).find("windowSwitcher/osd").attrib["style"], "thumbnail")
            link = config.with_name("managed.xml")
            link.symlink_to(config)
            with self.assertRaises(ValueError):
                alt_tab.install(link, config.read_bytes(), original)


if __name__ == "__main__":
    unittest.main()
