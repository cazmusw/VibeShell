"""Behavior checks driven by a virtual pointer on the private headless seat only."""
import json
from pathlib import Path
import select
import subprocess
import time
import tomllib


def check_transparency(args, env, ctl, wait):
    output = tomllib.loads(ctl('outputs').stdout)['outputs'][0]
    width, height = output['width'], output['height']

    def state():
        return tomllib.loads(ctl('dock', 'state').stdout)

    def patch(text):
        ctl('config', 'set', text)

    def capture(x, y, w, h):
        shot = subprocess.check_output(
            [args.grim, '-t', 'ppm', '-s', '1', '-g', f'{x},{y} {w}x{h}', '-'],
            env=env, timeout=5)
        magic, size, maximum, pixels = shot.split(b'\n', 3)
        assert (magic, size, maximum) == (b'P6', f'{w} {h}'.encode(), b'255')
        return pixels

    panel_fill = (width // 2, 8, 1, 1)
    dock_fill = (width // 2, height - 90, 1, 1)
    backdrop = (width // 2, 400, 1, 1)

    def check_half_transparent(region, opaque, wall):
        tint = wait(lambda: pixel if (pixel := capture(*region)) not in (wall, opaque) else None)
        assert all(min(a, b) <= c <= max(a, b) for a, b, c in zip(wall, opaque, tint))

    for theme, foreground, panel_color, dock_color in (
        ('dark', 'f3f5fa', '171b24', '222936'),
        ('light', '172438', 'f0f3f8', 'e9eef7'),
    ):
        wall = bytes.fromhex(foreground)
        wallpaper = subprocess.Popen([args.swaybg, '-c', f'#{foreground}'], env=env)
        try:
            wait(lambda: capture(*backdrop) == wall)
            patch(f"theme.mode='{theme}'\npanel.transparency_percent=0\ndock.transparency_percent=0")
            expected = bytes.fromhex(panel_color), bytes.fromhex(dock_color)
            wait(lambda: (capture(*panel_fill), capture(*dock_fill)) == expected)
            baseline = state()
            patch('panel.transparency_percent=50')
            check_half_transparent(panel_fill, expected[0], wall)
            assert capture(*dock_fill) == expected[1]
            patch('panel.transparency_percent=100')
            wait(lambda: capture(*panel_fill) == wall)
            assert capture(*dock_fill) == expected[1]
            # The wallpaper matches the nominal menu text color. The contrasting
            # outline must still provide visible edges around the glyphs.
            panel_text = capture(12, 9, 67, 17)
            assert any(abs(value - wall[channel % 3]) > 55
                       for channel, value in enumerate(panel_text))
            patch('dock.transparency_percent=50')
            check_half_transparent(dock_fill, expected[1], wall)
            assert capture(*panel_fill) == wall
            patch('dock.transparency_percent=100')
            wait(lambda: capture(*dock_fill) == wall)
            assert capture(*panel_fill) == wall
            dock_right = (width + baseline['width']) // 2
            launcher_icon = capture(dock_right - 58, height - 75, 32, 35)
            assert any(abs(value - wall[channel % 3]) > 55
                       for channel, value in enumerate(launcher_icon))
            for key in ('width', 'height', 'exclusive_zone', 'layout_revision'):
                assert state()[key] == baseline[key]
            assert tomllib.loads(ctl('config', 'get').stdout)['dock']['transparency_percent'] == 100
            preview = Path(args.report).parent / f'preview-transparency-{theme}.png'
            subprocess.run([args.grim, '-o', 'HEADLESS-1', str(preview)], env=env, check=True, timeout=5)
        finally:
            wallpaper.terminate()
            wallpaper.wait(timeout=3)
    patch("theme.mode='dark'\npanel.transparency_percent=0\ndock.transparency_percent=0")
    result = {'passed': True, 'independent_background_transparency': True,
              'foreground_contrast_in_both_themes': True}
    (Path(args.report).parent / 'surface-transparency.json').write_text(json.dumps(result) + '\n')
    print(json.dumps(result), flush=True)


def run(args, env, children, logs, ctl, wait, fixture, windows, config_file):
    def config():return tomllib.loads(ctl('config','get').stdout)
    def state():return tomllib.loads(ctl('dock','state').stdout)
    def patch(text):return tomllib.loads(ctl('config','set',text).stdout)
    result={}
    initial=config();assert initial['schema_version']==2
    before=initial['revision']
    bad=ctl('config','set','dock.icon_size=1',ok=False);assert bad.returncode!=0;assert config()['revision']==before
    patch("theme.mode='light'\ndock.icon_size=64")
    assert state()['icon_size']==64
    assert config()['theme']['mode']=='light'
    stale=subprocess.run(['busctl','--user','--timeout=3s','call','org.mosaic.Shell1','/org/mosaic/Shell1','org.mosaic.Shell1','ApplyPatch','ts',str(before),"theme.mode='dark'"],env=env,capture_output=True,text=True,timeout=5)
    assert stale.returncode!=0 and 'stale' in stale.stderr.lower(),stale.stderr
    original=config_file.read_text();config_file.write_text('invalid = [')
    assert ctl('reload',ok=False).returncode!=0
    assert config()['theme']['mode']=='light'
    config_file.write_text(original);ctl('reload')
    # A declared read-only source must remain untouched.
    managed=config_file.with_name('managed.toml');managed.write_text(config_file.read_text());config_file.unlink();config_file.symlink_to(managed)
    assert ctl('config','set',"theme.mode='dark'",ok=False).returncode!=0
    assert config()['theme']['mode']=='light'
    config_file.unlink();config_file.write_text(managed.read_text());ctl('reload')
    result['typed_patch_conflict_last_good_readonly']=True
    patch("pins=['mosaic-a.desktop','mosaic-b.desktop']\ntheme.mode='dark'\ndock.auto_hide=false\ndock.icon_size=48\ngeneral.reduced_motion=false\ndock.magnification=true")
    w=fixture();assert w
    time.sleep(.25)
    assert state()['width']>=280, 'Dock clips its short list after adding pins'
    mixed_width = state()['width']
    mixed_revision = state()['layout_revision']
    ctl('pin','mosaic-stage-0-probe.desktop')
    wait(lambda: state()['layout_revision'] > mixed_revision and state()['width'] < mixed_width)
    ctl('unpin','mosaic-stage-0-probe.desktop')
    wait(lambda: state()['width'] == mixed_width)
    result['mixed_groups_have_visible_divider']=True
    layout=state()['layout_revision']
    ctl('window','minimize',*w[:2]);wait(lambda:(f:=fixture()) and f[4]=='true')
    ctl('window','activate',*w[:2]);wait(lambda:(f:=fixture()) and f[3]=='true')
    assert state()['layout_revision']==layout
    result['stable_buttons_across_window_events']=True
    width,height=1280,720
    if args.scale!=1:
        width=int(2560/args.scale);height=int(1440/args.scale)
    pointer_log=(Path(config_file).parent/'pointer.log').open('w+')
    logs.append(pointer_log)
    pointer=subprocess.Popen([args.pointer,str(width),str(height)],env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=pointer_log,text=True,bufsize=1)
    children.append(pointer)
    def input(command):
        pointer.stdin.write(command+'\n');pointer.stdin.flush()
        assert select.select([pointer.stdout],[],[],3)[0], 'pointer command timed out'
        assert pointer.stdout.readline().strip()=='ok'
    def move(x,y):input(f'move {int(x)} {int(y)}')
    def button(code,down):input(f'button {code} {int(down)}')
    def first():
        s=state();return (width-s['width'])/2+52,height-12-s['height']+55
    def backdrop_top(dock):
        top = height - 12 - dock['height']
        pixel_count = min(50, dock['height'])
        shot = subprocess.check_output(
            [args.grim, '-t', 'ppm', '-s', '1', '-g',
             f'{width // 2},{top} 1x{pixel_count}', '-'], env=env, timeout=5)
        magic, size, maximum, pixels = shot.split(b'\n', 3)
        assert (magic, size, maximum) == (b'P6', f'1 {pixel_count}'.encode(), b'255')
        return next(row for row in range(pixel_count)
                    if pixels[3 * row:3 * row + 3] == bytes.fromhex('222936'))
    try:
        time.sleep(.25)
        x,y=first();move(x,y)
        wait(lambda:state()['max_scale']>1.15)
        assert state()['max_scale']<=1.45001
        time.sleep(.35);idle=state();assert not idle['animation_active']
        if args.grim:subprocess.run([args.grim,'-o','HEADLESS-1',str(Path(args.report).parent/'preview-zoom.png')],env=env,check=True,timeout=5)
        time.sleep(.3);assert state()['frames']==idle['frames']
        assert state()['layout_revision']==layout
        result['zoom_bounded_stable_and_no_idle_ticks']=True
        move(20,300)
        patch("dock.hover.zoom_percent=20\ndock.hover.radius_percent=110\ndock.hover.grow_ms=0\ndock.hover.shrink_ms=0\ndock.hover.easing='linear'")
        time.sleep(.2)
        x,y=first();move(x,y)
        wait(lambda: 1.1 < state()['max_scale'] <= 1.20001)
        move(20,300);wait(lambda: abs(state()['max_scale']-1)<.00001)
        patch("dock.hover.zoom_percent=70\ndock.hover.background_percent=25")
        wait(lambda: not state()['animation_active'])
        baseline = state()
        if args.grim:
            base_top = backdrop_top(baseline)
        x,y=first();move(x,y)
        wait(lambda: state()['max_scale'] > 1.60)
        enlarged = state()
        for key in ('width', 'height', 'exclusive_zone', 'layout_revision'):
            assert enlarged[key] == baseline[key]
        if args.grim:
            assert backdrop_top(enlarged) < base_top
        patch('general.reduced_motion=true')
        wait(lambda: not state()['animation_active'])
        assert state()['max_scale'] == 1.
        if args.grim:
            assert backdrop_top(state()) == base_top
        move(20,300)
        patch('general.reduced_motion=false\ndock.hover.background_percent=12')
        result['background_zoom_keeps_geometry_and_respects_reduced_motion']=True
        patch("dock.hover.zoom_percent=45\ndock.hover.radius_percent=65\ndock.hover.grow_ms=200\ndock.hover.shrink_ms=200\ndock.hover.easing='ease_out'")
        time.sleep(.2)
        x,y=first();move(x,y)
        result['configurable_hover_scale_and_duration']=True
        patch('general.reduced_motion=true')
        assert state()['reduced_motion'];assert abs(state()['max_scale']-1)<.00001
        move(x+20,y);time.sleep(.3);assert abs(state()['max_scale']-1)<.00001
        patch('general.reduced_motion=false\ndock.auto_hide=true')
        move(20,300);wait(lambda:not state()['visible'])
        assert state()['exclusive_zone']==0
        move(width/2,height-1)
        try:wait(lambda:state()['visible'])
        except TimeoutError:
            print('edge failure state',state(),flush=True);raise
        x,y=first();move(x,y);time.sleep(.9);assert state()['visible']
        # Popup must keep the Dock alive while pointer is outside.
        button(273,True);button(273,False);time.sleep(.15);move(20,300);time.sleep(.9)
        assert state()['visible'], 'Dock hid under an open popup'
        button(272,True);button(272,False)
        wait(lambda:not state()['visible'])
        result['edge_reveal_delay_and_menu_hold']=True
        move(width/2,height-1);wait(lambda:state()['visible'])
        x,y=first();move(x+62,y);button(272,True);move(x+42,y);time.sleep(.12);move(20,300);time.sleep(.85)
        assert state()['visible'],'Dock hid during drag'
        move(x,y);time.sleep(.2);button(272,False)
        wait(lambda:config()['pins'][:2]==['mosaic-b.desktop','mosaic-a.desktop'])
        result['real_drag_reorder_and_drag_hold']=True
        move(20,300);patch('dock.auto_hide=false\ndock.magnification=false')
        assert state()['visible']
        time.sleep(.2)
        # Drop on the trailing half of the adjacent pin, before the running-app section.
        x,y=first();move(x-12,y);button(272,True);move(x-32,y);time.sleep(.2)
        move(20,300);time.sleep(.15)
        move(x+65,y);time.sleep(.25);button(272,False)
        expected=['mosaic-a.desktop','mosaic-b.desktop']
        wait(lambda:config()['pins']==expected)
        assert tomllib.loads(config_file.read_text())['pins']==expected
        result['adjacent_pin_trailing_drop_persisted']=True
        if args.scale == 1:
            move(x+48,y);button(273,True);button(273,False);time.sleep(.22)
            if args.grim:
                subprocess.run([args.grim,'-o','HEADLESS-1',str(Path(args.report).parent/'preview-dock-menu.png')],
                               env=env,check=True,timeout=5)
            move(x+48,y-169);button(272,True);button(272,False)
            result['unpin_from_context_menu']=True
        else:
            ctl('unpin','mosaic-b.desktop')
        wait(lambda:config()['pins']==['mosaic-a.desktop'])
        assert tomllib.loads(config_file.read_text())['pins']==['mosaic-a.desktop']
        ctl('pin','mosaic-b.desktop')
        wait(lambda:config()['pins']==expected)
        result['unpin_persisted']=True
        w=fixture();ctl('window','activate',*w[:2]);wait(lambda:(f:=fixture()) and f[3]=='true')
        ctl('window','fullscreen',*w[:2]);wait(lambda:not state()['visible'])
        ctl('dock','show');wait(lambda:state()['visible'])
        ctl('window','unfullscreen',*w[:2])
        result['fullscreen_and_explicit_reveal']=True
        patch("theme.mode='light'\ngeneral.reduced_motion=true")
        ctl('settings');wait(lambda:tomllib.loads(ctl('dialog','state').stdout)['mapped'])
        if args.grim:
            time.sleep(.2);subprocess.run([args.grim,'-o','HEADLESS-1',str(Path(args.report).parent/'preview-light.png')],env=env,check=True,timeout=5)
        ctl('dialog','close');wait(lambda:not tomllib.loads(ctl('dialog','state').stdout)['mapped'])
        patch("theme.mode='system'");assert config()['theme']['mode']=='system'
        patch("theme.mode='dark'\ngeneral.reduced_motion=false\ndock.auto_hide=false\ndock.magnification=true")
    finally:
        if pointer.poll() is None:
            pointer.stdin.write('quit\n');pointer.stdin.flush();pointer.wait(timeout=3)
    result['passed']=True
    (Path(args.report).parent/'dock-behavior.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result),flush=True)
