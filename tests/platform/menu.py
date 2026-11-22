"""Real DBusMenu callbacks and GTK input on private bus/compositor only."""
import json,subprocess,time,tomllib,select
from pathlib import Path

def run(args,env,children,logs,ctl,wait,backend,spawn,window_fixture):
    def ui():return tomllib.loads(ctl('tray','state').stdout)
    def menu():return ui()['menu']
    def snapshot():return tomllib.loads(ctl('tray','snapshot').stdout)
    def admin(method,*params):
        r=subprocess.run(['busctl','--user','--timeout=4s','call','org.mosaic.TrayFixture','/Fixture','org.mosaic.TrayFixture',method,*params],env=env,capture_output=True,text=True,timeout=5)
        if r.returncode:raise RuntimeError(r.stderr)
        return r.stdout
    def start():
        log=(Path(args.report).parent/('menu-fixture-'+str(len(children))+'.log')).open('w+');logs.append(log)
        f=subprocess.Popen([args.tray_fixture],env=env,stdout=log,stderr=log);children.append(f)
        wait(lambda:subprocess.run(['busctl','--user','call','org.freedesktop.DBus','/org/freedesktop/DBus','org.freedesktop.DBus','NameHasOwner','s','org.mosaic.TrayFixture'],env=env,capture_output=True,text=True).stdout.strip()=='b true')
        admin('DBusMenu','b','true');admin('Register','s','/StatusNotifierItem');wait(lambda:len(snapshot().get('items',[]))==1)
        return f,snapshot()['items'][0]['key']
    wait(lambda:ui()['available']);fixture,key=start()
    model=tomllib.loads(ctl('tray','menu',key).stdout);assert model['available']
    token=model['token']
    for ident in [0,3,4,13,999]:assert ctl('tray','menu-event',key,str(token),str(ident),ok=False).returncode!=0
    assert ctl('tray','menu-event',key,str(token-1),'1',ok=False).returncode!=0
    plog=(Path(args.report).parent/'menu-pointer.log').open('w+');logs.append(plog)
    width,height=(1280,720) if args.scale==1 else (int(2560/args.scale),int(1440/args.scale))
    pointer=subprocess.Popen([args.pointer,str(width),str(height)],env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=plog,text=True,bufsize=1);children.append(pointer)
    def input(text):
        pointer.stdin.write(text+'\n');pointer.stdin.flush();assert select.select([pointer.stdout],[],[],3)[0];assert pointer.stdout.readline().strip()=='ok'
    def tray_point():
        wait(lambda:any(o['panel_mapped'] for o in tomllib.loads(ctl('outputs').stdout).get('outputs',[])))
        wait(lambda:len(ui().get('buttons',[]))==1 and ui()['buttons'][0]['width']>=20 and ui()['buttons'][0]['mapped'])
        input('move 20 200')
        def hovered():
            b=ui()['buttons'][0];input(f"move {int(b['x']+b['width']/2)} {int(b['y']+b['height']/2)}")
            return ui()['buttons'][0]['hovered']
        wait(hovered)
        return ui()['buttons'][0]
    def open():
        clicked=tray_point();input('button 273 1');input('button 273 0')
        try:wait(lambda:menu()['open'] and not menu()['loading'] and any(r['id']==1 and r['width']>0 for r in menu()['rows']))
        except TimeoutError:
            print('menu-open diagnostic',json.dumps({'clicked':clicked,'current':ui(),'outputs':tomllib.loads(ctl('outputs').stdout)}),flush=True);raise
    def row(ident):return next((r for r in menu()['rows'] if r['id']==ident and r['width']>0),None)
    def click(ident):
        r=wait(lambda:row(ident))
        def hovered():
            current=row(ident)
            if current is None:return False
            input(f"move {int(current['x']+current['width']/2)} {int(current['y']+current['height']/2)}")
            current=row(ident)
            return current and current['hovered']
        if r['enabled']:wait(hovered)
        else:input(f"move {int(r['x']+r['width']/2)} {int(r['y']+r['height']/2)}")
        input('button 272 1');input('button 272 0')
    def keyboard(*keys):subprocess.run([args.wtype,'-s','100',*sum((['-k',key] for key in keys),[])],env=env,check=True,timeout=5)
    admin('MenuUpdate','s','bad');tray_point();input('button 273 1');input('button 273 0')
    wait(lambda:menu()['open'] and bool(menu()['error']));assert menu()['keyboard_grabbed'];keyboard('Escape');wait(lambda:not menu()['open']);assert not menu()['keyboard_grabbed'];admin('MenuUpdate','s','normal')
    open();assert row(1)['width']>=200, 'Native menu labels are clipped by an undersized popover'
    assert menu()['keyboard_grabbed'];assert not row(4);assert not row(3)['enabled']
    before=admin('Events');click(3);time.sleep(.1);assert 'MenuEvent:3:clicked' not in admin('Events');assert menu()['open']
    click(1);wait(lambda:'MenuEvent:1:clicked' in admin('Events'));wait(lambda:not menu()['open']);assert not menu()['keyboard_grabbed']
    assert 'ContextMenu:' not in admin('Events')
    admin('FailEvent','b','true');open();click(1);wait(lambda:not menu()['open']);wait(lambda:bool(menu()['error']));admin('FailEvent','b','false')
    open();click(2);wait(lambda:'MenuEvent:2:clicked' in admin('Events'));wait(lambda:not menu()['open'])
    open();assert row(2)['toggle_state']==1
    admin('HoldLayout','b','true');count=admin('Events').count('GetLayout');admin('MenuUpdate','s','label');wait(lambda:admin('Events').count('GetLayout')>count)
    click(6);admin('MenuUpdate','s','removed');admin('HoldLayout','b','false');admin('ReleaseLayout')
    wait(lambda:menu()['parent']==6 and row(9));assert 'AboutToShow:6' in admin('Events')
    click(9);wait(lambda:'MenuEvent:9:clicked' in admin('Events'));wait(lambda:not menu()['open'])
    open();click(6);wait(lambda:menu()['parent']==6);keyboard('Left');wait(lambda:menu()['parent']==0);keyboard('Escape');wait(lambda:not menu()['open'])
    open();count=admin('Events').count('MenuEvent:2:clicked');keyboard('Down','Return');wait(lambda:admin('Events').count('MenuEvent:2:clicked')>count);wait(lambda:not menu()['open'])
    open();old_token=menu()['token'];admin('MenuUpdate','s','label');wait(lambda:row(1) and row(1)['label']=='Updated menu')
    admin('Title','s','Changed SNI title');wait(lambda:'Changed SNI title' in ui()['buttons'][0]['tooltip']);assert menu()['open']
    admin('MenuUpdate','s','disabled');wait(lambda:row(1) and not row(1)['enabled'])
    assert ctl('tray','menu-event',key,str(old_token),'1',ok=False).returncode!=0
    admin('MenuUpdate','s','removed');wait(lambda:not row(10));assert menu()['open']
    if args.grim:subprocess.run([args.grim,'-o','HEADLESS-1',str(Path(args.report).parent/'preview-menu.png')],env=env,check=True,timeout=5)
    # Invalidate a reply which is deliberately held by the fixture.
    admin('HoldLayout','b','true');count=admin('Events').count('GetLayout');admin('MenuUpdate','s','normal');wait(lambda:admin('Events').count('GetLayout')>count)
    admin('MenuUpdate','s','removed');admin('HoldLayout','b','false');admin('ReleaseLayout');wait(lambda:menu()['open'] and not menu()['loading'] and not row(10) and not menu()['error'])
    admin('MenuUpdate','s','bad');wait(lambda:bool(menu()['error']));assert not menu()['rows']
    admin('MenuUpdate','s','normal');wait(lambda:row(1) and not menu()['error']);count=admin('Events').count('GetLayout');time.sleep(.25);assert admin('Events').count('GetLayout')==count
    keyboard('Escape');wait(lambda:not menu()['open'])
    open();w=window_fixture();ctl('window','fullscreen',*w[:2]);wait(lambda:not menu()['open']);assert not menu()['keyboard_grabbed'];ctl('window','unfullscreen',*w[:2])
    open();subprocess.run([args.randr,'--output','HEADLESS-1','--off'],env=env,check=True,timeout=5);wait(lambda:not menu()['open']);assert not menu()['keyboard_grabbed']
    subprocess.run([args.randr,'--output','HEADLESS-1','--on'],env=env,check=True,timeout=5);wait(lambda:len(ui().get('buttons',[]))==1)
    open();previous=menu()['token'];backend.kill();backend.wait(timeout=5);wait(lambda:not ui()['available']);assert not menu()['open'] and not menu()['keyboard_grabbed']
    backend=spawn(args.services);wait(lambda:ui()['available']);admin('Register','s','/StatusNotifierItem');wait(lambda:len(snapshot().get('items',[]))==1)
    newer=tomllib.loads(ctl('tray','menu',key).stdout);assert newer['token']!=previous
    assert ctl('tray','menu-event',key,str(previous),'1',ok=False).returncode!=0
    admin('HoldLayout','b','true');count=admin('Events').count('GetLayout')
    tray_point();input('button 273 1');input('button 273 0')
    wait(lambda:admin('Events').count('GetLayout')>count);admin('Quit');fixture.wait(timeout=5);wait(lambda:not menu()['open']);assert not menu()['keyboard_grabbed']
    pointer.stdin.write('quit\n');pointer.stdin.flush();pointer.wait(timeout=3)
    result={name:True for name in ['passed','native_pointer_menu_actions','checkbox_and_lazy_submenu','keyboard_escape_arrows_enter','live_properties_and_layout','stale_tokens_and_delayed_reply','malformed_menu_rejected','no_idle_polling','owner_backend_output_loss_closed_menu','initial_error_visible','queued_lazy_preparation','event_failure_visible']}
    (Path(args.report).parent/'dbusmenu.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result),flush=True)
    return backend
