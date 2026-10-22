#!/usr/bin/env python3
"""Create an encrypted archive; never persist plaintext beyond the temporary job directory."""
import hashlib
import base64
import ssl
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid
import resource
import signal

MAX_BYTES = 32 * 1024 * 1024
RECIPIENT = 'age1zc8k5aqm3qwv42l3r29kfuraxtm68yesfxw2swlhufskk6mkg4ms0tr8ht'


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise urllib.error.HTTPError(req.full_url, code, 'Backup redirects are forbidden', headers, fp)


def upload_client(root):
    cert = root / 'client.pem'
    key = root / 'client.key'
    try:
        cert.write_bytes(base64.b64decode(os.environ['MTLS_CLIENT_CERT_B64'], validate=True))
        key.write_bytes(base64.b64decode(os.environ['MTLS_CLIENT_KEY_B64'], validate=True))
        cert.chmod(0o600)
        key.chmod(0o600)
        context = ssl.create_default_context()
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(cert, key)
        return urllib.request.build_opener(urllib.request.HTTPSHandler(context=context), NoRedirect())
    finally:
        cert.unlink(missing_ok=True)
        key.unlink(missing_ok=True)


def main():
    os.umask(0o077)
    url = os.environ['BACKUP_URL'].rstrip('/')
    if not url.startswith('https://'):
        raise RuntimeError('HTTPS is required')
    os.environ['PGOPTIONS'] = '-c default_transaction_read_only=on -c statement_timeout=180000 -c lock_timeout=5000'
    with tempfile.TemporaryDirectory(prefix='vc-backup-') as directory:
        root = Path(directory)
        client = upload_client(root)
        def bound_output():
            resource.setrlimit(resource.RLIMIT_FSIZE, (MAX_BYTES, MAX_BYTES))
        subprocess.run([
            'pg_dump', '--no-password', '--format=custom', '--compress=gzip:6',
            '--lock-wait-timeout=5s', f'--file={root / "database.dump"}',
        ], check=True, timeout=300, preexec_fn=bound_output)
        dump = root / 'database.dump'
        if not 0 < dump.stat().st_size <= MAX_BYTES:
            raise RuntimeError('Dump exceeds size limit')
        encrypted = root / 'database.dump.age'
        subprocess.run(['age', '-r', RECIPIENT, '-o', str(encrypted), str(dump)], check=True, timeout=60)
        dump.unlink()
        if encrypted.stat().st_size > MAX_BYTES:
            raise RuntimeError('Encrypted backup exceeds size limit')
        body = encrypted.read_bytes()
        digest = hashlib.sha256(body).hexdigest()
        backup_id = str(uuid.uuid4())
        # Retry only ambiguous transport errors, always with the same ID and body.
        # Never retry 4xx or generate a new ID to bypass an admission failure.
        for attempt in range(3):
            request = urllib.request.Request(url + '/upload', data=body, method='POST', headers={
                'Content-Type': 'application/octet-stream',
                'User-Agent': 'virtualcrypto-backup/1.0',
                'X-Backup-Id': backup_id, 'X-Backup-Sha256': digest,
            })
            try:
                with client.open(request, timeout=150) as response:
                    receipt = json.loads(response.read(4096))
                if receipt.get('sha256') != digest or receipt.get('bytes') != len(body):
                    raise RuntimeError('Invalid upload receipt')
                print(json.dumps(receipt))
                return
            except urllib.error.HTTPError:
                raise
            except (urllib.error.URLError, TimeoutError):
                if attempt == 2:
                    raise
                time.sleep(10)


def notify_failure():
    webhook = os.environ.get('DISCORD_WEBHOOK_URL')
    if not webhook:
        return
    payload = json.dumps({'content': 'virtualCrypto: 定期バックアップが失敗しました。Fly の virtualcrypto-backup のログを確認してください。', 'allowed_mentions': {'parse': []}}).encode()
    request = urllib.request.Request(webhook, data=payload, headers={'Content-Type': 'application/json', 'User-Agent': 'virtualcrypto-backup/1.0'})
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            response.read(1024)
    except Exception:
        print('Backup failure notification could not be delivered', flush=True)


if __name__ == '__main__':
    def interrupted(_signum, _frame):
        raise RuntimeError('Backup process timed out or was terminated')
    signal.signal(signal.SIGTERM, interrupted)
    try:
        main()
    except Exception:
        notify_failure()
        raise
