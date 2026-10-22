#!/usr/bin/env python3
"""Update the existing scheduled Machine without starting an extra backup."""
import argparse
import copy
import os
import json
import re
import time
from urllib.request import Request, urlopen

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


def api(path='', *, method='GET', data=None, nonce=None):
    headers = {'Authorization': 'Bearer ' + os.environ['FLY_API_TOKEN'], 'Content-Type': 'application/json'}
    if nonce:
        headers['fly-machine-lease-nonce'] = nonce
    request = Request(f'https://api.machines.dev/v1/apps/{APP}/machines/{path}'.rstrip('/'),
                      data=json.dumps(data).encode() if data is not None else None,
                      headers=headers, method=method)
    with urlopen(request, timeout=60) as response:
        body = response.read()
        return json.loads(body) if body else None


def machines():
    return api()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('image')
    args = parser.parse_args()
    if not re.fullmatch(r'registry\.fly\.io/virtualcrypto-backup@sha256:[0-9a-f]{64}', args.image):
        raise RuntimeError('Expected immutable backup-app registry image digest')
    before = validate_machine(machines())
    print('Previous image:', before['config']['image'], flush=True)
    lease_path = before['id'] + '/lease'
    lease = api(lease_path, method='POST', data={'ttl': 300, 'description': 'vc3 backup image update'})
    nonce = lease['data']['nonce']
    try:
        current = validate_machine([api(before['id'])])
        if current['instance_id'] != before['instance_id']:
            raise RuntimeError('Machine changed while acquiring lease; retry deployment')
        config = copy.deepcopy(before['config'])
        config['image'] = args.image
        api(before['id'], method='POST', nonce=nonce, data={
            'config': config, 'current_version': before['instance_id'], 'skip_launch': True,
        })
    finally:
        api(lease_path, method='DELETE', nonce=nonce)
    deadline = time.monotonic() + 120
    while True:
        observed = machines()
        if len(observed) != 1 or observed[0]['id'] != before['id']:
            raise RuntimeError('Machine inventory changed')
        if observed[0]['state'] == 'stopped':
            after = validate_machine(observed)
            break
        if observed[0]['state'] not in ('created', 'updating', 'pending', 'replacing') or time.monotonic() >= deadline:
            raise RuntimeError(f"Image update did not settle to a stopped Machine: {observed[0]['state']}")
        time.sleep(2)
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
