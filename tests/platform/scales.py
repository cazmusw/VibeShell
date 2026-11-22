"""Real headless Labwc/GTK protocol smoke test; no claims about physical GPU QA."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--labwc', required=True)
parser.add_argument('--randr', required=True)
parser.add_argument('--probe', required=True)
parser.add_argument('--report', required=True)
args = parser.parse_args()
results = []
with tempfile.TemporaryDirectory(prefix='mosaic-scale-') as directory:
    root = Path(directory)
    runtime, config = root/'runtime', root/'config'
    runtime.mkdir(mode=0o700)
    config.mkdir()
    (config/'rc.xml').write_text('<labwc_config/>\n')
    (config/'autostart').write_text('')
    (config/'environment').write_text('')
    env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), WAYLAND_DISPLAY='wayland-0', WLR_BACKENDS='headless', WLR_HEADLESS_OUTPUTS='1', WLR_RENDERER='pixman', GDK_BACKEND='wayland', GSK_RENDERER='cairo', GTK_A11Y='none')
    env.pop('WAYLAND_SOCKET', None)
    with (root/'compositor.log').open('w+') as log:
        compositor = subprocess.Popen([args.labwc, '--config-dir', str(config)], env=env, stdout=log, stderr=log)
        try:
            deadline = time.monotonic()+5
            while not (runtime/'wayland-0').exists():
                if compositor.poll() is not None or time.monotonic()>deadline:
                    log.seek(0)
                    raise RuntimeError('headless startup failed: '+log.read())
                time.sleep(0.02)
            for scale in [1, 1.25, 1.5, 2]:
                subprocess.run([args.randr, '--output', 'HEADLESS-1', '--scale', str(scale)], env=env, capture_output=True, check=True, timeout=5)
                outputs = json.loads(subprocess.run([args.randr, '--json'], env=env, capture_output=True, text=True, check=True, timeout=5).stdout)
                assert outputs[0]['scale'] == scale, outputs
                layer = subprocess.run([args.probe], env=env, capture_output=True, text=True, check=True, timeout=8)
                match = re.search(r'surface_scale=([0-9.]+)', layer.stdout)
                assert match and float(match[1]) == scale, layer.stdout
                assert 'surface mapped=true, allocation=480x40' in layer.stdout, layer.stdout
                assert 'keyboard_active=false' in layer.stdout, layer.stdout
                control = subprocess.run([args.probe, '--window-control'], env=env, capture_output=True, text=True, check=True, timeout=8)
                assert 'restored and activated confirmed' in control.stdout, control.stdout
                results.append({'scale':scale,'backend':'headless/pixman','renderer':'cairo','layer_surface':layer.stdout.strip(),'window_control':control.stdout.strip(),'stderr':layer.stderr+control.stderr,'passed':True})
                print(f'scale {scale}: mapped, size, fractional negotiation and window control passed', flush=True)
        finally:
            compositor.terminate()
            try:
                compositor.wait(timeout=5)
            except subprocess.TimeoutExpired:
                compositor.kill()
                compositor.wait(timeout=3)
Path(args.report).write_text(json.dumps({'fixture':'Labwc 0.9.7 without XWayland','physical_gpu_test':False,'results':results},ensure_ascii=False,indent=2)+'\n')
