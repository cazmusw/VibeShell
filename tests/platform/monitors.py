"""Real multi-output lifecycle on a private Labwc; never touches host outputs."""
import json,select,subprocess,tomllib
from pathlib import Path

def run(args,env,ctl,wait,fixture):
    def topology():return tomllib.loads(ctl('outputs').stdout)['outputs']
    def change(*args_):subprocess.run([args.randr,*args_],env=env,check=True,timeout=6)
    def selected():return [o['connector'] for o in topology() if o['dock_selected']]
    def panels():return [o for o in topology() if o['panel_mapped']]
    def patch(text):ctl('config','set',text)
    def dock_state(connector=None):
        return tomllib.loads(ctl('dock','state',*([connector] if connector else [])).stdout)
    def config():return tomllib.loads(ctl('config','get').stdout)
    def dialog():return tomllib.loads(ctl('dialog','state').stdout)
    def shown_on(kind, connector):
        state=dialog()
        return state['mapped'] and state['kind']==kind and state['connector']==connector
    def close_dialog():
        ctl('dialog','close')
        wait(lambda:not dialog()['mapped'])
    def output_of(w):
        rows=[line.split('\t') for line in ctl('window-outputs').stdout.splitlines()]
        row=next((r for r in rows if r[:2]==w[:2]),None)
        return row[2:] if row else []
    wait(lambda:len(panels())==2)
    assert selected()==['HEADLESS-1']
    all_=topology();assert {o['connector'] for o in all_}=={'HEADLESS-1','HEADLESS-2'}
    assert any(o['x']<0 for o in all_)
    assert sorted(o['scale'] for o in all_)==[1.0,1.5]
    for o in all_:assert o['panel_width']==o['width'] and o['panel_height']>0
    pins=tomllib.loads(ctl('config','get').stdout)['pins']
    patch("dock.preferred_output='HEADLESS-2'")
    wait(lambda:selected()==['HEADLESS-2'])
    assert sum(o['dock_mapped'] for o in topology())==1
    w=fixture();names=wait(lambda:output_of(w));assert len(names)==1,names
    ctl('window','fullscreen',*w[:2])
    wait(lambda:len(panels())==1)
    assert {o['connector'] for o in panels()}==({'HEADLESS-1','HEADLESS-2'}-set(names))
    dock=tomllib.loads(ctl('dock','state').stdout)
    assert dock['visible']==('HEADLESS-2' not in names)
    ctl('window','unfullscreen',*w[:2]);wait(lambda:len(panels())==2)
    ctl('window','activate',*w[:2])
    wait(lambda:fixture()[3]=='true')
    active=wait(lambda:output_of(w))[0]
    patch(f"general.borderless_fullscreen_apps=[{w[2]}]")
    wait(lambda: {o['connector'] for o in panels()} == {'HEADLESS-1', 'HEADLESS-2'} - {active})
    assert dock_state()['visible'] == (active != 'HEADLESS-2')
    patch("general.borderless_fullscreen_apps=[]")
    wait(lambda: len(panels()) == 2)
    other=next(o for o in topology() if o['connector']!=active)
    patch(f"dock.preferred_output='{other['connector']}'")
    # The virtual pointer speaks layout-relative coordinates. Mosaic only
    # receives a GTK click on the button of the selected panel.
    outputs=topology()
    left=min(o['x'] for o in outputs)
    top=min(o['y'] for o in outputs)
    width=max(o['x']+o['width'] for o in outputs)-left
    height=max(o['y']+o['height'] for o in outputs)-top
    pointer=subprocess.Popen([args.pointer,str(width),str(height)],env=env,
                             stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
    try:
        def send(command):
            pointer.stdin.write(command+'\n')
            pointer.stdin.flush()
            assert select.select([pointer.stdout],[],[],3)[0]
            assert pointer.stdout.readline().strip()=='ok'
        # Introduce the virtual pointer before the dialog checks so GTK
        # binds wl_pointer before the first press on the panel.
        send('move 20 200')
        ctl('settings')
        wait(lambda:shown_on('settings',active))
        close_dialog()
        ctl('control-center')
        wait(lambda:shown_on('control-center',active))
        ctl('control-center',other['connector'])
        wait(lambda:shown_on('control-center',other['connector']))
        close_dialog()
        ctl('window','minimize',*w[:2])
        wait(lambda:fixture()[4]=='true')
        ctl('settings')
        wait(lambda:shown_on('settings',other['connector']))
        close_dialog()
        ctl('window','activate',*w[:2])
        wait(lambda:fixture()[3]=='true')
        send(f"move {other['x']-left+30} {other['y']-top+16}")
        send('button 272 1')
        send('button 272 0')
        try:
            wait(lambda:shown_on('launcher',other['connector']))
        except TimeoutError as error:
            raise AssertionError(f"Panel click on {other['connector']} produced {dialog()} with outputs {outputs}") from error
        subprocess.run([args.grim,'-o',other['connector'],
                        str(Path(args.report).parent/'preview-dialog.png')],env=env,check=True,timeout=5)
        subprocess.run([args.wtype,'-s','100','-k','Escape'],env=env,check=True,timeout=5)
        wait(lambda:not dialog()['mapped'])
        ctl('settings')
        wait(lambda:dialog()['mapped'])
        ctl('dock','show')
        wait(lambda:not dialog()['mapped'])
        assert tomllib.loads(ctl('dock','state').stdout)['visible']

        def key(name):
            subprocess.run([args.wtype,'-s','100','-k',name],env=env,check=True,timeout=5)
        def hover_dock(connector):
            output=next(o for o in topology() if o['connector']==connector)
            dock=wait(lambda: (state if (state:=dock_state(connector))['width']>0 else None))
            x=output['x']-left+(output['width']-dock['width'])//2+52
            y=output['y']-top+output['height']-12-dock['height']+55
            send(f"move {x} {y}")
        def cards(connector=None):
            return ctl('dock','cards',*([connector] if connector else [])).stdout.split()

        key('Escape')
        ctl('window','activate',*w[:2])
        wait(lambda:fixture()[3]=='true')
        assert config()['dock']['current_output_only'] is False
        patch(f"dock.preferred_output='{active}'\ndock.current_output_only=true")
        wait(lambda:selected()==[active])
        ctl('unpin','mosaic-stage-0-probe.desktop')
        hover_dock(active)
        wait(lambda:cards()==[w[1]])
        local_layout=dock_state()['layout_revision']
        key('F8' if other['connector']=='HEADLESS-1' else 'F9')
        wait(lambda:output_of(w)==[other['connector']])
        wait(lambda:not cards() and dock_state()['layout_revision']>local_layout)

        filtered_layout=dock_state()['layout_revision']
        patch('dock.current_output_only=false')
        wait(lambda:dock_state()['layout_revision']>filtered_layout)
        send('move 20 200')
        hover_dock(active)
        wait(lambda:cards()==[w[1]])
        ctl('pin','mosaic-stage-0-probe.desktop')
        wait(lambda:not cards())
        pinned_layout=dock_state()['layout_revision']
        send('move 20 200')
        hover_dock(active)
        wait(lambda:cards()==[w[1]])
        send('move 20 200')
        patch('dock.current_output_only=true')
        wait(lambda:not cards())
        assert dock_state()['layout_revision']==pinned_layout
        assert config()['pins']==pins
        from cards import indicator_marks
        def marks():
            output=next(o for o in topology() if o['connector']==active)
            return indicator_marks(args,env,output,dock_state(active))
        wait(lambda:marks()==0)

        def launched_window():
            for line in ctl('windows').stdout.splitlines():
                fields=line.split('\t')
                if fields[2]=='"mosaic-stage-0-probe"' and fields[:2]!=w[:2]:
                    return fields[:2]
            return None
        hover_dock(active)
        send('button 272 1')
        send('button 272 0')
        opened=wait(launched_window)
        ctl('window','close',*opened)
        wait(lambda:not launched_window())
        ctl('window','activate',*w[:2])
        wait(lambda:fixture()[3]=='true')

        key('F8' if active=='HEADLESS-1' else 'F9')
        wait(lambda:output_of(w)==[active])
        send('move 20 200')
        hover_dock(active)
        wait(lambda:cards()==[w[1]])
        assert dock_state()['layout_revision']==pinned_layout
        send('move 20 200')
        wait(lambda:marks()==1)
        send('move 20 200')
        patch('dock.current_output_only=false')

        remote = other['connector']
        assert not config()['dock']['all_outputs']
        patch('dock.all_outputs=true\ndock.current_output_only=true')
        wait(lambda:sum(o['dock_mapped'] for o in topology())==2)
        assert selected()==[active]
        ctl('unpin','mosaic-stage-0-probe.desktop')
        wait(lambda:dock_state(active)['width']>dock_state(remote)['width']>0)

        dock = dock_state(remote)
        grid_x = other['x']-left+(other['width']+dock['width'])//2-42
        grid_y = other['y']-top+other['height']-12-dock['height']//2
        send(f"move {grid_x} {grid_y}")
        send('button 272 1')
        send('button 272 0')
        wait(lambda:shown_on('launcher',remote))
        close_dialog()
        ctl('window','activate',*w[:2])
        wait(lambda:fixture()[3]=='true')
        key('F8' if remote=='HEADLESS-1' else 'F9')
        wait(lambda:output_of(w)==[remote])
        wait(lambda:dock_state(remote)['width']>dock_state(active)['width']>0)
        hover_dock(remote)
        wait(lambda:cards(remote)==[w[1]])
        assert not cards()
        send('move 20 200')
        ctl('window','fullscreen',*w[:2])
        wait(lambda:not dock_state(remote)['visible'] and dock_state(active)['visible'])
        ctl('window','unfullscreen',*w[:2])
        wait(lambda:sum(o['dock_mapped'] for o in topology())==2)
        ctl('pin','mosaic-stage-0-probe.desktop')
        assert config()['pins']==pins

        patch('dock.auto_hide=true')
        wait(lambda:not dock_state(active)['visible'] and not dock_state(remote)['visible'])
        assert ctl('dock','show','missing',ok=False).returncode!=0
        assert not dock_state(active)['visible'] and not dock_state(remote)['visible']
        ctl('dock','show',remote)
        wait(lambda:dock_state(remote)['visible'])
        assert not dock_state(active)['visible']
        key('Escape')
        patch('dock.auto_hide=false')
        wait(lambda:sum(o['dock_mapped'] for o in topology())==2)
        revisions = {name:dock_state(name)['layout_revision'] for name in (active,remote)}
        patch(f"dock.preferred_output='{remote}'")
        wait(lambda:selected()==[remote] and sum(o['dock_mapped'] for o in topology())==2)
        assert all(dock_state(name)['layout_revision']==revision for name,revision in revisions.items())
        patch('dock.all_outputs=false')
        wait(lambda:sum(o['dock_mapped'] for o in topology())==1)
        assert ctl('dock','state',active,ok=False).returncode!=0
        patch('dock.all_outputs=true\ndock.current_output_only=false')
        wait(lambda:sum(o['dock_mapped'] for o in topology())==2)
    finally:
        pointer.terminate()
        pointer.wait(timeout=3)

    # Close shell-owned dialogs before their output disappears.
    patch("dock.preferred_output='HEADLESS-2'")
    ctl('control-center','HEADLESS-2')
    wait(lambda:shown_on('control-center','HEADLESS-2'))
    change('--output','HEADLESS-2','--off')
    wait(lambda:len(panels())==1 and selected()==['HEADLESS-1'])
    wait(lambda:not dialog()['mapped'])
    assert len(topology())==1
    assert tomllib.loads(ctl('config','get').stdout)['dock']['preferred_output']=='HEADLESS-2'
    assert tomllib.loads(ctl('config','get').stdout)['pins']==pins
    for _ in range(2):
        change('--output','HEADLESS-2','--on','--scale','1.5','--pos','0,0')
        wait(lambda:len(panels())==2 and selected()==['HEADLESS-2'])
        wait(lambda:sum(o['dock_mapped'] for o in topology())==2)
        change('--output','HEADLESS-2','--off');wait(lambda:len(panels())==1 and selected()==['HEADLESS-1'])
    patch('dock.all_outputs=false')
    change('--output','HEADLESS-1','--scale','2')
    wait(lambda:topology()[0]['scale']==2.0)
    wait(lambda:topology()[0]['panel_width']==topology()[0]['width'])
    assert tomllib.loads(ctl('dock','state').stdout)['width']<=topology()[0]['width']
    change('--output','HEADLESS-1','--off')
    wait(lambda:len(topology())==0)
    assert not tomllib.loads(ctl('dock','state').stdout)['visible']
    # The output management head survives disabled wl_output globals.
    change('--output','HEADLESS-1','--on','--scale','1','--pos','0,0')
    wait(lambda:len(panels())==1 and selected()==['HEADLESS-1'])
    patch("dock.preferred_output='missing'");assert selected()==['HEADLESS-1']
    assert tomllib.loads(ctl('config','get').stdout)['pins']==pins
    patch("dock.preferred_output=''")
    result={'passed':True,'two_panels_one_dock':True,'mixed_scales_negative_placement':True,
            'preferred_fallback_reconnect':True,'pins_preserved':True,'dialog_closed_on_disconnect':True,
            'fullscreen_scoped_to_output':True,'zero_outputs_recovered':True,'scale_change_resized':True,
            'cli_dialog_prefers_active_then_dock_output':True,'panel_click_uses_own_output':True,
            'escape_closes_dialog':True,'dock_focus_dismisses_dialog':True,
            'dock_filter_follows_window_output_events':True,'dock_filter_keeps_pins':True,
            'dock_filter_scopes_cards':True,'dock_filter_restores_global_scope':True,
            'remote_only_pin_launches':True,'all_outputs_live_toggle':True,
            'second_dock_routes_launcher':True,'all_outputs_local_scope':True,
            'all_outputs_independent_reveal_and_fullscreen':True,'all_outputs_hotplug':True}
    (Path(args.report).parent/'monitors.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result),flush=True)
