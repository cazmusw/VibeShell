"""Exercise only uniquely named fixture units on the current user bus."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--report', required=True)
args = parser.parse_args()
base = ['busctl', '--user', '--timeout=5s', '--']
manager = ['org.freedesktop.systemd1', '/org/freedesktop/systemd1',
           'org.freedesktop.systemd1.Manager']


def call(method, *arguments):
    result = subprocess.run(base + ['call', *manager, method, *arguments],
                            capture_output=True, text=True, timeout=7)
    result.check_returncode()
    return result.stdout


def object_for(unit):
    return call('GetUnit', 's', unit).split('"')[1]


def prop(unit, interface, name):
    return subprocess.run(base + ['get-property', 'org.freedesktop.systemd1',
                                 object_for(unit), interface, name],
                          check=True, capture_output=True, text=True,
                          timeout=7).stdout.strip()


def pid(unit):
    return int(prop(unit, 'org.freedesktop.systemd1.Service', 'MainPID').split()[1])


def absent(error):
    return any(text in (error.stderr or '') for text in
               ['not loaded', 'not found', 'No such file or directory'])


def wait_until(test, seconds=5):
    deadline = time.monotonic() + seconds
    while not test():
        if time.monotonic() > deadline:
            raise TimeoutError('lifecycle condition deadline')
        time.sleep(.05)


with tempfile.TemporaryDirectory(prefix='mosaic-lifecycle-') as workspace:
    key = uuid.uuid4().hex[:10]
    target = f'mosaic-stage0-{key}.target'
    services = [f'mosaic-stage0-{key}-{name}.service' for name in ['ui', 'backend']]
    created = []
    result = {'test': 'runtime-linked user target with transient sleep services; '
                      'not production components', 'passed': False}
    cleanup_errors = []

    def stopped(unit):
        try:
            return prop(unit, 'org.freedesktop.systemd1.Unit', 'ActiveState') == 's \"inactive\"'
        except subprocess.CalledProcessError as error:
            if absent(error):
                return True
            raise
    try:
        target_file = Path(workspace) / target
        target_file.write_text('[Unit]\nDescription=Mosaic isolated lifecycle test target\n')
        created.append(target)
        call('LinkUnitFiles', 'asbb', '1', str(target_file), 'true', 'false')
        call('Reload')
        assert 'linked-runtime' in call('GetUnitFileState', 's', target)
        call('StartUnit', 'ss', target, 'fail')
        sleep = shutil.which('sleep')
        if sleep is None:
            raise RuntimeError('sleep not found')
        for service in services:
            created.append(service)
            call('StartTransientUnit', 'ssa(sv)a(sa(sv))', service, 'fail', '8',
                 'Description', 's', 'Mosaic lifecycle fixture only',
                 'Type', 's', 'simple',
                 'ExecStart', 'a(sasb)', '1', sleep, '2', sleep, '600', 'false',
                 'PartOf', 'as', '1', target,
                 'BindsTo', 'as', '1', target,
                 'After', 'as', '1', target,
                 'Restart', 's', 'on-failure',
                 'RestartUSec', 't', '100000', '0')
        wait_until(lambda: all(pid(service) > 0 for service in services))
        old_ui, old_backend = [pid(service) for service in services]
        call('KillUnit', 'ssi', services[0], 'main', '9')
        wait_until(lambda: pid(services[0]) not in [0, old_ui])
        assert pid(services[1]) == old_backend
        result['restart_after_failure_confirmed'] = True
        result['backend_survived_ui_failure'] = True
        result['restart_count'] = prop(services[0], 'org.freedesktop.systemd1.Service', 'NRestarts')
        call('StopUnit', 'ss', target, 'replace')

        wait_until(lambda: all(stopped(service) for service in services))
        result['target_stop_stopped_both_services'] = True
        result['behavior_passed'] = True
    finally:
        # Every cleanup attempt runs even if an earlier D-Bus operation times out.
        for unit in reversed(created):
            try:
                call('StopUnit', 'ss', unit, 'replace')
            except subprocess.CalledProcessError as error:
                if not absent(error):
                    cleanup_errors.append(str(error.stderr))
            except (subprocess.TimeoutExpired, OSError) as error:
                cleanup_errors.append(str(error))
        for unit in created:
            try:
                wait_until(lambda: stopped(unit))
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired,
                    TimeoutError, OSError) as error:
                cleanup_errors.append('stop verification: ' + str(error))
        if target in created:
            for method, arguments in [
                ('DisableUnitFiles', ('asb', '1', target, 'true')),
                ('Reload', ()),
            ]:
                try:
                    call(method, *arguments)
                except subprocess.CalledProcessError as error:
                    if not absent(error):
                        cleanup_errors.append(str(error.stderr))
                except (subprocess.TimeoutExpired, OSError) as error:
                    cleanup_errors.append(str(error))
            try:
                state = call('GetUnitFileState', 's', target)
                cleanup_errors.append('runtime target still registered: ' + state)
            except subprocess.CalledProcessError as error:
                if not absent(error):
                    cleanup_errors.append(str(error.stderr))
                else:
                    result['runtime_target_unlinked'] = True
            except (subprocess.TimeoutExpired, OSError) as error:
                cleanup_errors.append(str(error))
        result['passed'] = bool(result.get('behavior_passed')) and not cleanup_errors
        if cleanup_errors:
            result['cleanup_errors'] = cleanup_errors
        Path(args.report).write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
    if cleanup_errors:
        raise RuntimeError('fixture cleanup failed: ' + repr(cleanup_errors))
    print(json.dumps(result), flush=True)
