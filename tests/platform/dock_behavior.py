"""Behavior checks driven by a virtual pointer on the private headless seat only."""
import json
from pathlib import Path
import select
import subprocess
import time
import tomllib


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
