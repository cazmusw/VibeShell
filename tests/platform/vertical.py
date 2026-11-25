"""Launch the real UI/backend on an isolated bus and headless compositor."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import tomllib

p=argparse.ArgumentParser()
for name in ['labwc','shell','services','ctl','probe','report']:p.add_argument('--'+name,required=True)
p.add_argument('--inside',action='store_true')
p.add_argument('--grim')
p.add_argument('--swaybg')
p.add_argument('--screenshot')
p.add_argument('--session')
p.add_argument('--interrupt-session',action='store_true')
p.add_argument('--tray-external',action='store_true')
p.add_argument('--overflow',action='store_true')
p.add_argument('--calendar',action='store_true')
p.add_argument('--control-center',action='store_true')
p.add_argument('--launcher-check', action='store_true')
p.add_argument('--alt-tab')
p.add_argument('--files')
p.add_argument('--files-trash')
p.add_argument('--mime-data')
p.add_argument('--pulseaudio')
p.add_argument('--pactl')
p.add_argument('--pacat')
p.add_argument('--hardware-fixture',action='store_true')
p.add_argument('--wifi-fixture')
p.add_argument('--bluetooth-fixture')
p.add_argument('--brightness-fixture')
p.add_argument('--power-fixture')
p.add_argument('--temperatures', action='store_true')
p.add_argument('--cards',action='store_true')
p.add_argument('--live-cards',action='store_true')
p.add_argument('--dbusmenu',action='store_true')
p.add_argument('--wtype')
p.add_argument('--tray',action='store_true')
p.add_argument('--tray-fixture')
p.add_argument('--monitors',action='store_true')
p.add_argument('--dock-behavior',action='store_true')
p.add_argument('--dock-edges',action='store_true')
p.add_argument('--drop-fixture')
p.add_argument('--gvfsd')
p.add_argument('--gio')
p.add_argument('--confirm-trash-delete',action='store_true')
p.add_argument('--pointer')
p.add_argument('--randr')
p.add_argument('--scale',type=float,default=1)
a=p.parse_args()
if not a.inside:
    with tempfile.TemporaryDirectory(prefix='mosaic-private-bus-') as bus_dir:
        config=Path(bus_dir)/'bus.conf'
        config.write_text('<busconfig><type>session</type><listen>unix:tmpdir='+bus_dir+'</listen><auth>EXTERNAL</auth><policy context="default"><allow own="*"/><allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>')
        raise SystemExit(subprocess.call(['dbus-run-session','--config-file='+str(config),'--',sys.executable,__file__,*sys.argv[1:],'--inside']))

def wait(check,seconds=12):
    end=time.monotonic()+seconds
    while True:
        value=check()
        if value:return value
        if time.monotonic()>end:raise TimeoutError('vertical slice condition timed out')
        time.sleep(.05)

with tempfile.TemporaryDirectory(prefix='mosaic-vertical-') as directory:
    root=Path(directory);runtime=root/'runtime';runtime.mkdir(mode=0o700)
    cfg=root/'labwc';cfg.mkdir();(cfg/'rc.xml').write_text('<labwc_config/>\n');(cfg/'autostart').write_text('');(cfg/'environment').write_text('')
    if a.files:
        (cfg/'rc.xml').write_text(
            '<labwc_config><windowRules>'
            '<windowRule identifier="org.mosaic.TransferReceiver">'
            '<action name="MoveTo" x="930" y="400" />'
            '<action name="ToggleAlwaysOnTop" />'
            '</windowRule></windowRules></labwc_config>\n')
    if a.alt_tab:
        subprocess.run([sys.executable, a.alt_tab, '--config', cfg/'rc.xml', '--apply'],
                       check=True, timeout=5)
    if a.monitors:
        (cfg/'rc.xml').write_text(
            '<labwc_config><keyboard>'
            '<keybind key="F8"><action name="MoveToOutput" output="HEADLESS-1" /></keybind>'
            '<keybind key="F9"><action name="MoveToOutput" output="HEADLESS-2" /></keybind>'
            '</keyboard></labwc_config>\n')
    apps=root/'share/applications';apps.mkdir(parents=True)
    (apps/'mosaic-stage-0-probe.desktop').write_text('[Desktop Entry]\nType=Application\nName=Mosaic fixture\nExec="'+a.probe+'" --hold-window\nIcon=application-x-executable\n')
    if a.launcher_check:
        import launcher_check
        launcher_check.prepare(apps, root, sys.executable, a.probe)
    if a.files:
        from shutil import copyfile
        desktop = Path(__file__).resolve().parents[2] / 'assets/org.mosaic.Files.desktop'
        copyfile(desktop, apps / desktop.name)
    if a.interrupt_session:
        from interrupt_session import desktop_files
        desktop_files(apps,root,sys.executable,a.probe)
    if a.dock_behavior:
        for ident,name in [('mosaic-a.desktop','Alpha'),('mosaic-b.desktop','Beta')]:
            (apps/ident).write_text('[Desktop Entry]\nType=Application\nName='+name+'\nExec=\"'+a.probe+'\" --hold-window\nIcon=application-x-executable\n')
    if a.drop_fixture:
        marker = root / 'unexpected-launch'
        canary = root / 'launch-canary.py'
        canary.write_text(f'from pathlib import Path\nPath({str(marker)!r}).touch()\n')
        launcher = apps / 'tools/editor.desktop'
        launcher.parent.mkdir()
        launcher.write_text(f'[Desktop Entry]\nType=Application\nName=Drop test editor\nExec="{sys.executable}" "{canary}"\n')
    config=root/'config/mosaic-shell';config.mkdir(parents=True)
    config_file=config/'config.toml';config_file.write_text('schema_version=1\nrevision=0\npins=[]\n')
    env=dict(os.environ,XDG_RUNTIME_DIR=str(runtime),WAYLAND_DISPLAY='wayland-0',WLR_BACKENDS='headless',WLR_HEADLESS_OUTPUTS='2' if a.monitors else '1',WLR_RENDERER='pixman',GDK_BACKEND='wayland',GSK_RENDERER='cairo',GTK_A11Y='none',GSETTINGS_BACKEND='memory',GTK_USE_PORTAL='0',XDG_CONFIG_HOME=str(root/'config'),XDG_CACHE_HOME=str(root/'cache'),XDG_DATA_HOME=str(root/'share'),XDG_DATA_DIRS=str(root/'empty'))
    env.pop('WAYLAND_SOCKET',None)
    env['XDG_STATE_HOME'] = str(root/'state')
    if a.files:
        env['PATH'] = f'{Path(a.files).parent}:{env["PATH"]}'
    if a.control_center or a.wifi_fixture or a.bluetooth_fixture or a.brightness_fixture or a.power_fixture:
        env['DBUS_SYSTEM_BUS_ADDRESS']=env['DBUS_SESSION_BUS_ADDRESS']
    if a.bluetooth_fixture:
        env['MOSAIC_BLUETOOTH_PAIRING']='1'
    if a.brightness_fixture:
        import brightness_check
        brightness_check.prepare(env, root)
    if os.environ.get('MOSAIC_WAYLAND_TRACE'):env['WAYLAND_DEBUG']='client'
    if a.gvfsd:
        home=root/'home';home.mkdir()
        env['HOME']=str(home)
        uri_marker=root/'trash-opened'
        handler=root/'trash-handler.py'
        handler.write_text(f'from pathlib import Path\nimport sys\nPath({str(uri_marker)!r}).write_text(sys.argv[1])\n')
        (apps/'mosaic-trash-handler.desktop').write_text(
            f'[Desktop Entry]\nType=Application\nName=Trash handler fixture\n'
            f'Exec="{sys.executable}" "{handler}" %u\nMimeType=x-scheme-handler/trash;\n')
        (root/'config/mimeapps.list').write_text(
            '[Default Applications]\nx-scheme-handler/trash=mosaic-trash-handler.desktop;\n')
    children=[];logs=[]
    def spawn(exe):
        log=(root/(Path(exe).name+str(len(children))+'.log')).open('w+')
        logs.append(log);child=subprocess.Popen([exe],env=env,stdout=log,stderr=log);children.append(child);return child
    def ctl(*args,ok=True):
        result=subprocess.run([a.ctl,*args],env=env,capture_output=True,text=True,timeout=5)
        if ok and result.returncode:raise RuntimeError(result.stderr)
        return result
    def windows():
        result=ctl('windows',ok=False)
        if result.returncode:return []
        return [line.split('\t') for line in result.stdout.splitlines()]
    def fixture():return next((w for w in windows() if w[2]=='"mosaic-stage-0-probe"'),None)
    def bus(name,path,interface,method):
        return subprocess.run(['busctl','--user','--timeout=2s','call',name,path,interface,method],env=env,capture_output=True,text=True,timeout=4)
    try:
        log=(root/'labwc.log').open('w+');logs.append(log)
        compositor=subprocess.Popen([a.labwc,'--config-dir',str(cfg)],env=env,stdout=log,stderr=log);children.append(compositor)
        wait(lambda:(runtime/'wayland-0').exists())
        if a.monitors:
            subprocess.run([a.randr,'--output','HEADLESS-1','--custom-mode','1280x720@60Hz','--pos=-1280,0','--output','HEADLESS-2','--custom-mode','1920x1080@60Hz','--scale','1.5','--pos','0,0'],env=env,check=True,timeout=6)
        if a.overflow:
            subprocess.run([a.randr,"--output","HEADLESS-1","--custom-mode","640x480@60Hz"],env=env,check=True,timeout=5)
        if a.scale!=1:
            subprocess.run([a.randr,'--output','HEADLESS-1','--custom-mode','2560x1440@60Hz','--scale',str(a.scale)],env=env,check=True,timeout=5)
        external_fixture=None
        if a.tray_external:
            log=(root/'external-watcher.log').open('w+');logs.append(log)
            external_fixture=subprocess.Popen([a.tray_fixture,'--watcher'],env=env,stdout=log,stderr=log);children.append(external_fixture)
            wait(lambda:bus('org.mosaic.TrayFixture','/Fixture','org.mosaic.TrayFixture','Events').returncode==0)
        if a.gvfsd:
            log=(root/'gvfsd.log').open('w+');logs.append(log)
            daemon=subprocess.Popen([a.gvfsd,'--no-fuse'],env=env,stdout=log,stderr=log)
            children.append(daemon)
            wait(lambda:bus('org.freedesktop.DBus','/org/freedesktop/DBus',
                            'org.freedesktop.DBus','ListNames').stdout.find('org.gtk.vfs.Daemon')>=0)
        if a.pulseaudio:
            from audio_check import start
            pulse=start(a,env,root,children,logs,wait)
        if a.hardware_fixture:
            from hardware_pactl import initial_state
            fixture_path=root/'audio-hardware.json'
            initial_state(fixture_path)
            env['MOSAIC_HARDWARE_FIXTURE']=str(fixture_path)
        backend=spawn(a.services);ui=spawn(a.shell)
        wait(lambda:bus('org.mosaic.Shell1','/org/mosaic/Shell1','org.mosaic.Shell1','GetHealth').returncode==0)
        if a.dock_behavior:
            wait(lambda:tomllib.loads(ctl('dock','state').stdout)['width']>0)
            assert tomllib.loads(ctl('dock','state').stdout)['layout_revision']>0, 'Empty Dock did not construct its Applications button'
        assert bus('org.mosaic.Services1','/org/mosaic/Services1','org.mosaic.Services1','GetSnapshot').returncode==0
        for name,leaf in [('Shell1','shell1.xml'),('Services1','services1.xml')]:
            xml=subprocess.run(['busctl','--user','--xml-interface','introspect','org.mosaic.'+name,'/org/mosaic/'+name],env=env,capture_output=True,text=True,check=True,timeout=5)
            (Path(a.report).parent/leaf).write_text(xml.stdout)
        duplicate=subprocess.run([a.shell],env=env,capture_output=True,timeout=5);assert duplicate.returncode!=0
        duplicate_backend=subprocess.run([a.services],env=env,capture_output=True,timeout=5);assert duplicate_backend.returncode!=0
        assert '\tshell\tdbus_start_failed\t1' in ctl('logs').stdout
        assert '\tservices\texit\t1' in ctl('logs').stdout
        def dialog():
            return tomllib.loads(ctl('dialog','state').stdout)
        for command in ['launcher','settings']:
            ctl(command)
            wait(lambda:dialog()['mapped'] and dialog()['kind']==command)
            if command=='launcher' and a.grim and a.screenshot:
                time.sleep(.2)
                subprocess.run([a.grim,'-o','HEADLESS-1',a.screenshot],env=env,check=True,timeout=5)
            ctl('dialog','close')
            wait(lambda:not dialog()['mapped'])
        if a.launcher_check:
            launcher_check.run(ctl, wait, fixture, root)
            w = wait(fixture)
        else:
            ctl('launch','mosaic-stage-0-probe.desktop');w=wait(fixture)
        epoch,ident=w[:2]
        ctl('window','minimize',epoch,ident);wait(lambda:(f:=fixture()) and f[4]=='true')
        ctl('window','activate',epoch,ident);wait(lambda:(f:=fixture()) and f[3]=='true' and f[4]=='false')
        ctl('pin','mosaic-stage-0-probe.desktop')
        wait(lambda:'mosaic-stage-0-probe.desktop' in tomllib.loads(config_file.read_text())['pins'])
        if a.overflow:
            from overflow_check import run
            run(a,env,children,logs,ctl,wait)
        if a.calendar:
            from calendar_check import run
            run(a,env,children,logs,ctl,wait)
        if a.control_center:
            from control_center_check import run
            run(a,env,children,ctl,wait,config_file)
        if a.temperatures:
            from temperatures_check import run
            run(a,env,children,ctl,wait)
        if a.alt_tab:
            from alt_tab_check import run
            run(a,env,children,ctl,wait,windows,(epoch,ident))
        if a.pulseaudio:
            from audio_check import run
            run(a,env,root,children,logs,ctl,wait,pulse)
        if a.pacat:
            from playback_check import run
            run(a,env,children,ctl,wait)
        if a.hardware_fixture:
            from hardware_check import run
            run(a,env,fixture_path,ctl,wait)
        if a.cards:
            from cards import run
            run(a,env,children,logs,ctl,wait,windows)
        if a.dbusmenu:
            from menu import run
            backend=run(a,env,children,logs,ctl,wait,backend,spawn,fixture)
        if a.tray:
            from tray import run
            run(a,env,children,logs,ctl,wait,external_fixture)
            if not a.tray_external:
                backend.kill();backend.wait(timeout=5)
                wait(lambda:not tomllib.loads(ctl('tray','state').stdout)['available'])
                backend=spawn(a.services)
                run(a,env,children,logs,ctl,wait)
                report=Path(a.report).parent/'tray.json';data=json.loads(report.read_text());data['backend_restart_recovered']=True;report.write_text(json.dumps(data,indent=2)+'\n')
        if a.monitors:
            from monitors import run
            run(a,env,ctl,wait,fixture)
        if a.dock_behavior:
            from dock_behavior import run
            run(a,env,children,logs,ctl,wait,fixture,windows,config_file)
        if a.dock_edges:
            from dock_edges import run
            run(a,env,children,ctl,wait)
        if a.drop_fixture:
            from desktop_drop import run
            run(a,env,children,logs,ctl,wait,config_file,launcher,marker)
        if a.gvfsd:
            from trash_check import run
            run(a,env,children,ctl,wait,root,uri_marker)
        ctl('window','close',epoch,ident);wait(lambda:not fixture())
        assert ctl('window','activate',epoch,ident,ok=False).returncode!=0
        if a.files_trash:
            from trash_check import files_trash
            files_trash(a,env,children,logs,ctl,wait,root,windows)
        if a.cards and a.grim:
            from cards import check_indicators
            check_indicators(a,env,ctl,wait,windows)
        if a.swaybg:
            from dock_behavior import check_transparency
            check_transparency(a,env,ctl,wait)
        if a.dock_behavior and a.grim and a.scale in (1, 1.5):
            from dock_behavior import check_icon_rendering
            check_icon_rendering(a,env,ctl,wait,config_file)
        if a.live_cards:
            from live_cards import run
            run(a,env,root,children,logs,ctl,wait,windows)
        if a.files:
            from file_manager_check import run
            run(a,env,root,children,logs,ctl,wait,windows)
        if a.wifi_fixture:
            from wifi_check import run
            run(a,env,children,ctl,wait,spawn)
        if a.bluetooth_fixture:
            from bluetooth_check import run
            run(a,env,children,ctl,wait,spawn)
        if a.brightness_fixture:
            brightness_check.run(a,env,children,ctl,wait,spawn)
        if a.power_fixture:
            from power_check import run
            run(a,env,ctl,wait,spawn)
        ui.kill();ui.wait(timeout=5);assert backend.poll() is None
        ui=spawn(a.shell)
        wait(lambda:bus('org.mosaic.Shell1','/org/mosaic/Shell1','org.mosaic.Shell1','GetHealth').returncode==0)
        ctl('launch','mosaic-stage-0-probe.desktop');new=wait(fixture);assert new[0]!=epoch
        assert ctl('window','activate',epoch,ident,ok=False).returncode!=0
        ctl('window','close',*new[:2]);wait(lambda:not fixture())
        ctl('quit');ui.wait(timeout=5);assert ui.returncode==0
        assert bus('org.mosaic.Services1','/org/mosaic/Services1','org.mosaic.Services1','Quit').returncode==0
        backend.wait(timeout=5);assert backend.returncode==0
        if a.interrupt_session:
            from interrupt_session import run
            def ui_ready():
                return bus('org.mosaic.Shell1','/org/mosaic/Shell1','org.mosaic.Shell1','GetHealth').returncode==0
            def backend_ready():
                return bus('org.mosaic.Services1','/org/mosaic/Services1','org.mosaic.Services1','GetHealth').returncode==0
            run(a,env,root,ctl,wait,windows,ui_ready,backend_ready)
        if a.session:
            session=spawn(a.session)
            wait(lambda:bus('org.mosaic.Shell1','/org/mosaic/Shell1','org.mosaic.Shell1','GetHealth').returncode==0)
            ctl('quit');session.wait(timeout=5);assert session.returncode==0
            wait(lambda:bus('org.mosaic.Services1','/org/mosaic/Services1','org.mosaic.Services1','GetHealth').returncode!=0)
            assert '\tsession\tui_exit\t0' in ctl('logs').stdout
        if a.session and a.files:
            from shutil import copyfile
            failing = root/'failing-launch'
            failing.mkdir()
            launcher = failing/'mosaic-session'
            copyfile(a.session, launcher)
            launcher.chmod(0o700)
            (failing/'shellctl').symlink_to(a.ctl)
            interpreter = launcher.read_text().splitlines()[0]
            for name, status in [('mosaic-shell', 42), ('mosaic-services', 0)]:
                fixture_script = failing/name
                fixture_script.write_text(f'{interpreter}\nexit {status}\n')
                fixture_script.chmod(0o700)
            assert subprocess.run([launcher], env=env, timeout=5).returncode == 42
            assert '\tsession\tui_exit\t42' in ctl('logs').stdout
        for log in logs:
            log.flush();log.seek(0);text=log.read()
            if Path(log.name).name.startswith(('mosaic-shell','mosaic-services')):
                assert not any(marker in text for marker in ['Gtk-WARNING','Gtk-CRITICAL','GLib-CRITICAL','panicked at']),text[-6000:]
        result={'passed':True,'real_gio_launch':True,'minimize_activate_confirmed':True,'closed_and_old_epoch_rejected':True,'pins_persisted':True,'ui_restart_backend_survived':True,'duplicate_ui_rejected':True,'isolated_bus_and_compositor':True,'manual_runner_cleanup':bool(a.session)}
        Path(a.report).write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result),flush=True)
    except Exception:
        for log in logs:
            log.flush();log.seek(0);text=log.read();print(text[-6000:],file=sys.stderr)
            if os.environ.get('MOSAIC_WAYLAND_TRACE') and Path(log.name).name.startswith('mosaic-shell'):Path(os.environ['MOSAIC_WAYLAND_TRACE']).write_text(text)
        raise
    finally:
        for child in reversed(children):
            if child.poll() is None:child.terminate()
        for child in reversed(children):
            try:child.wait(timeout=5)
            except subprocess.TimeoutExpired:child.kill();child.wait(timeout=3)
        for log in logs:log.close()
