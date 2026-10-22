#!/usr/bin/env python3
"""Update the existing scheduled Machine without starting an extra backup."""
import argparse
import json
import re
import subprocess

APP = 'virtualcrypto-backup'


def validate_machine(machines):
    if len(machines) != 1:
        raise RuntimeError('Expected exactly one backup Machine; refusing to create or remove Machines')
    machine = machines[0]
    config = machine['config']
    if machine['state'] != 'stopped':
        raise RuntimeError('Backup Machine is not stopped; retry after the current backup finishes')
    if config.get('schedule') != 'daily' or config.get('restart', {}).get('policy') != 'no':
        raise RuntimeError('Unexpected schedule/restart configuration')
    if config.get('services') or config.get('mounts'):
        raise RuntimeError('Backup Machine must not expose services or mount volumes')
    if config.get('guest') != {'cpu_kind': 'shared', 'cpus': 1, 'memory_mb': 256}:
        raise RuntimeError('Unexpected backup compute allocation')
    if config.get('rootfs') != {'persist': 'never', 'size_gb': 2}:
        raise RuntimeError('Unexpected backup root filesystem configuration')
    return machine


def machines():
    return json.loads(subprocess.check_output(['flyctl', 'machine', 'list', '--app', APP, '--json'], text=True))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('image')
    args = parser.parse_args()
    if not re.fullmatch(r'registry\.fly\.io/virtualcrypto-backup@sha256:[0-9a-f]{64}', args.image):
        raise RuntimeError('Expected immutable backup-app registry image digest')
    before = validate_machine(machines())
    print('Previous image:', before['config']['image'], flush=True)
    subprocess.run(['flyctl', 'machine', 'update', before['id'], '--app', APP,
                    '--image', args.image, '--skip-start', '--yes'], check=True)
    after = validate_machine(machines())
    if after['id'] != before['id']:
        raise RuntimeError('Machine identity changed')
    if after['config']['image'].split('@')[-1] != args.image.split('@')[-1]:
        raise RuntimeError('Machine did not retain the requested image digest')
    # The workflow pushes a single-platform image without an attestation index.
    # Compare all non-image fields to preserve scheduling and credentials.
    old = {k: v for k, v in before['config'].items() if k not in ('image', 'metadata')}
    new = {k: v for k, v in after['config'].items() if k not in ('image', 'metadata')}
    if old != new:
        raise RuntimeError('Unexpected Machine configuration change; inspect before the next run')
    print('Scheduled image:', after['config']['image'])


if __name__ == '__main__':
    main()
