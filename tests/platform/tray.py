"""SNI fixture on the private bus, actual GTK button/pointer actions."""
import json,subprocess,time,tomllib,select
from pathlib import Path

def run(args,env,children,logs,ctl,wait,prestarted=None):
    def ui():return tomllib.loads(ctl('tray','state').stdout)
    def snapshot():return tomllib.loads(ctl('tray','snapshot').stdout)
    wait(lambda:ui()['available'])
    log=(Path(args.report).parent/'tray-fixture.log').open('w+');logs.append(log)
    fixture=prestarted
    if fixture is None:
        fixture=subprocess.Popen([args.tray_fixture],env=env,stdout=log,stderr=log);children.append(fixture)
    def admin(method,*params):
        p=subprocess.run(['busctl','--user','--timeout=4s','call','org.mosaic.TrayFixture','/Fixture','org.mosaic.TrayFixture',method,*params],env=env,capture_output=True,text=True,timeout=5)
        if p.returncode:raise RuntimeError(p.stderr)
        return p.stdout
    wait(lambda:subprocess.run(['busctl','--user','call','org.freedesktop.DBus','/org/freedesktop/DBus','org.freedesktop.DBus','NameHasOwner','s','org.mosaic.TrayFixture'],env=env,capture_output=True,text=True).stdout.strip()=='b true')
    def owner(name):return subprocess.check_output(['busctl','--user','call','org.freedesktop.DBus','/org/freedesktop/DBus','org.freedesktop.DBus','GetNameOwner','s',name],env=env,text=True,timeout=4)
    if prestarted is not None:
        wait(lambda:'HostRegistered' in admin('Events'))
        assert owner('org.kde.StatusNotifierWatcher')!=owner('org.mosaic.Tray1'), 'Existing watcher was replaced'

    admin('Register','s','org.example.TrayAlias')
    wait(lambda:len(snapshot().get('items',[]))==1)
    admin('DropAlias');wait(lambda:not snapshot().get('items',[]))
    admin('Register','s','/StatusNotifierItem')
    wait(lambda:len(snapshot().get('items',[]))==1)
    admin('Register','s','org.mosaic.TrayFixture');time.sleep(.1);assert len(snapshot()['items'])==1
    wait(lambda:len(ui().get('buttons',[]))==1)
    item=snapshot()['items'][0];assert item['pixels']==[255,0,0,255,0,255,0,128]
    assert ui()['buttons'][0]['has_icon']
    admin('Status','s','NeedsAttention');wait(lambda:ui()['buttons'][0]['attention'])
    admin('Title','s','Updated <b>literal</b>');wait(lambda:'Updated <b>literal</b>' in ui()['buttons'][0]['tooltip'])
    admin('Status','s','Passive');wait(lambda:len(ui().get('buttons',[]))==0)
    admin('Status','s','Active');wait(lambda:len(ui().get('buttons',[]))==1)
    pointerlog=(Path(args.report).parent/'tray-pointer.log').open('w+');logs.append(pointerlog)
    pointer=subprocess.Popen([args.pointer,'1280','720'],env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=pointerlog,text=True,bufsize=1);children.append(pointer)
    def input(command):
        pointer.stdin.write(command+'\n');pointer.stdin.flush();assert select.select([pointer.stdout],[],[],3)[0];assert pointer.stdout.readline().strip()=='ok'
    wait(lambda:len(ui().get('buttons',[]))==1 and ui()['buttons'][0]['width']>=20 and ui()['buttons'][0]['height']>0)
    b=ui()['buttons'][0];input(f"move {int(b['x']+b['width']/2)} {int(b['y']+b['height']/2)}")
    for code in [272,274,273]:input(f'button {code} 1');input(f'button {code} 0')
    input('axis v -10');input('axis h 10')
    wait(lambda:all(name in admin('Events') for name in ['Activate','SecondaryActivate','ContextMenu','Scroll']))
    wait(lambda:'horizontal' in admin('Events'))
    events=admin('Events');assert '120' in events and 'vertical' in events and 'horizontal' in events,events
    admin('Menu','b','true');wait(lambda:snapshot()['items'][0]['item_is_menu'])
    revision=snapshot()['revision'];wait(lambda:ui()['revision']>=revision and ui()['buttons'][0]['width']>=20 and ui()['buttons'][0]['height']>0)
    b=ui()['buttons'][0];input('move 20 200');input(f"move {int(b['x']+b['width']/2)} {int(b['y']+b['height']/2)}")
    input('button 272 1');input('button 272 0')
    wait(lambda:admin('Events').count('ContextMenu')>=2)
    if args.grim:subprocess.run([args.grim,'-o','HEADLESS-1',str(Path(args.report).parent/'preview-tray.png')],env=env,check=True,timeout=5)
    admin('Register','s','/Other');wait(lambda:len(ui()['buttons'])==2)
    # Owner removal also cancels the meaning of previously shown buttons.
    admin('Quit');fixture.wait(timeout=5);wait(lambda:len(ui().get('buttons',[]))==0)
    assert not snapshot().get('items',[])
    if prestarted is not None:wait(lambda:owner('org.kde.StatusNotifierWatcher')==owner('org.mosaic.Tray1'))
    pointer.stdin.write('quit\n');pointer.stdin.flush();pointer.wait(timeout=3)
    result={'passed':True,'registration_forms_and_dedup':True,'argb_icon_rendered':True,'signals_attention_tooltip_passive':True,'actual_pointer_actions_and_scroll':True,'multiple_paths_owner_cleanup':True,'alias_release_cleanup':True,'external_watcher_host':prestarted is not None}
    (Path(args.report).parent/'tray.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result),flush=True)
