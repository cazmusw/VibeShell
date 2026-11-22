"""Private-bus watcher races/provenance; no display or host services."""
import argparse,json,os,subprocess,sys,tempfile,time,tomllib
from pathlib import Path
p=argparse.ArgumentParser()
for key in ['services','ctl','fixture','report']:p.add_argument('--'+key,required=True)
p.add_argument('--inside',action='store_true');p.add_argument('--scenario',default='all')
a=p.parse_args()
if not a.inside:
    with tempfile.TemporaryDirectory(prefix='mosaic-tray-registry-') as d:
        c=Path(d)/'bus.conf';c.write_text('<busconfig><type>session</type><listen>unix:tmpdir='+d+'</listen><auth>EXTERNAL</auth><policy context="default"><allow own="*"/><allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>')
        raise SystemExit(subprocess.call(['dbus-run-session','--config-file='+str(c),'--',sys.executable,__file__,*sys.argv[1:],'--inside']))
def wait(test):
    end=time.monotonic()+6
    while True:
        v=test()
        if v:return v
        if time.monotonic()>end:raise TimeoutError('tray registry condition')
        time.sleep(.02)
with tempfile.TemporaryDirectory(prefix='mosaic-tray-registry-') as d:
    env=dict(os.environ,XDG_RUNTIME_DIR=d);children=[];logs=[];results={}
    def spawn(exe,*args):
        log=(Path(d)/(str(len(children))+'.log')).open('w+');logs.append(log)
        proc=subprocess.Popen([exe,*args],env=env,stdout=log,stderr=log);children.append(proc);return proc
    def call(name,method,*args):
        r=subprocess.run(['busctl','--user','--timeout=3s','call',name,'/Fixture','org.mosaic.TrayFixture',method,*args],env=env,capture_output=True,text=True,timeout=4)
        if r.returncode:raise RuntimeError(r.stderr)
        return r.stdout
    def has(name):return subprocess.check_output(['busctl','--user','call','org.freedesktop.DBus','/org/freedesktop/DBus','org.freedesktop.DBus','NameHasOwner','s',name],env=env,text=True).strip()=='b true'
    def owner(name):return subprocess.check_output(['busctl','--user','call','org.freedesktop.DBus','/org/freedesktop/DBus','org.freedesktop.DBus','GetNameOwner','s',name],env=env,text=True).strip()
    def items():
        r=subprocess.run([a.ctl,'tray','snapshot'],env=env,capture_output=True,text=True,timeout=4)
        return tomllib.loads(r.stdout).get('items',[]) if r.returncode==0 else None
    def count(n):return items() is not None and len(items())==n
    def cleanup():
        for c in children:
            if c.poll() is None:c.kill()
        for c in children:c.wait(timeout=5)
        children.clear();wait(lambda:not has('org.mosaic.Tray1') and not has('org.mosaic.TrayFixture'))
    try:
        if a.scenario in ['all','unregister']:
            spawn(a.fixture,'--watcher');wait(lambda:has('org.mosaic.TrayFixture'))
            spawn(a.services);wait(lambda:has('org.mosaic.Tray1'));wait(lambda:'HostRegistered' in call('org.mosaic.TrayFixture','Events'))
            for value in ['org.mosaic.TrayFixture/StatusNotifierItem','org.mosaic.TrayFixture']:
                call('org.mosaic.TrayFixture','Add','s',value);wait(lambda:count(1))
                call('org.mosaic.TrayFixture','Unregister','s',value);wait(lambda:count(0))
            results['external_unregistration_aliases']=True;cleanup()
        if a.scenario in ['all','provenance']:
            spawn(a.fixture,'--watcher=kde');wait(lambda:has('org.mosaic.TrayFixture'))
            spawn(a.fixture,'--watcher=fd','--admin=org.mosaic.SecondWatcher');wait(lambda:has('org.mosaic.SecondWatcher'))
            spawn(a.services);wait(lambda:has('org.mosaic.Tray1'))
            for service in ['org.mosaic.TrayFixture','org.mosaic.SecondWatcher']:wait(lambda:'HostRegistered' in call(service,'Events'))
            # Canonical alias makes old unregistration match, isolating provenance.
            unique=owner('org.mosaic.TrayFixture').split('"')[1];value=unique+'/StatusNotifierItem'
            call('org.mosaic.TrayFixture','Add','s',value);wait(lambda:count(1))
            call('org.mosaic.SecondWatcher','Add','s',value);time.sleep(.2)
            call('org.mosaic.TrayFixture','ReleaseWatcher')
            wait(lambda:owner('org.kde.StatusNotifierWatcher')==owner('org.mosaic.Tray1'))
            wait(lambda:count(1))
            call('org.mosaic.SecondWatcher','Unregister','s',value);wait(lambda:count(0))
            results['two_watcher_provenance']=True;cleanup()
        if a.scenario in ['all','stale']:
            spawn(a.fixture,'--watcher','--hold-host','--preload');wait(lambda:has('org.mosaic.TrayFixture'))
            spawn(a.services);wait(lambda:has('org.mosaic.Tray1'));wait(lambda:'HostRegistered' in call('org.mosaic.TrayFixture','Events'))
            call('org.mosaic.TrayFixture','ReleaseWatcher')
            wait(lambda:owner('org.kde.StatusNotifierWatcher')==owner('org.mosaic.Tray1'))
            call('org.mosaic.TrayFixture','ReleaseHost');time.sleep(.4)
            assert count(0), 'Old watcher response resurrected items'
            results['stale_watcher_reply_rejected']=True;cleanup()
        results['passed']=True;Path(a.report).write_text(json.dumps(results,indent=2)+'\n');print(json.dumps(results),flush=True)
    except Exception:
        for log in logs:log.flush();log.seek(0);print(log.read()[-4000:],file=sys.stderr)
        raise
    finally:
        for c in children:
            if c.poll() is None:c.kill()
        for c in children:c.wait(timeout=5)
