import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class SessionLifecycle(unittest.TestCase):
    def test_cleanup_clears_display_from_both_activation_environments(self):
        with tempfile.TemporaryDirectory() as d:
            directory=Path(d)
            (directory/'systemctl').write_text('''#!/bin/sh
printf '%s\\n' "systemctl $*" >> "$TEST_LOG"
case "$*" in *is-active*) exit 1;; esac
''')
            (directory/'dbus-update-activation-environment').write_text('''#!/bin/sh
printf '%s\\n' "dbus $*" >> "$TEST_LOG"
''')
            (directory/'labwc').write_text('#!/bin/sh\nexit 7\n')
            for p in directory.iterdir():p.chmod(0o755)
            log=directory/'commands'
            env={**os.environ,'PATH':f'{directory}:/usr/bin:/bin','TEST_LOG':str(log)}
            result=subprocess.run([str(ROOT/'scripts/mosaic-session')],env=env,capture_output=True)
            self.assertEqual(result.returncode,7,result.stderr)
            commands=log.read_text()
            self.assertIn('dbus --systemd WAYLAND_DISPLAY= DISPLAY= XDG_CURRENT_DESKTOP=',commands)
            self.assertIn('systemctl --user unset-environment WAYLAND_DISPLAY DISPLAY XDG_CURRENT_DESKTOP',commands)

if __name__=='__main__': unittest.main()
