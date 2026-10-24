import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('deploy', Path(__file__).parents[1] / 'deploy/update.py')
deploy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deploy)


class DeploymentTests(unittest.TestCase):
    def setUp(self):
        self.machine = {'id': 'existing', 'state': 'stopped', 'config': {
            'schedule': 'daily', 'restart': {'policy': 'no'},
            'guest': {'cpu_kind': 'shared', 'cpus': 1, 'memory_mb': 256},
            'rootfs': {'persist': 'never', 'size_gb': 2}}}

    def test_accepts_existing_bounded_job(self):
        self.assertEqual(deploy.validate_machine([self.machine])['id'], 'existing')

    def test_refuses_missing_extra_or_running_machines(self):
        for machines in ([], [self.machine, self.machine], [dict(self.machine, state='started')]):
            with self.subTest(machines=machines), self.assertRaises(RuntimeError):
                deploy.validate_machine(machines)

    def test_refuses_cost_or_exposure_drift(self):
        for key, value in [('schedule', 'hourly'), ('restart', {'policy': 'always'}),
                           ('guest', {'memory_mb': 1024}), ('rootfs', {'size_gb': 10}),
                           ('services', [{'ports': [80]}]), ('mounts', [{'volume': 'data'}])]:
            machine = copy.deepcopy(self.machine)
            machine['config'][key] = value
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                deploy.validate_machine([machine])

class UpdateTests(unittest.TestCase):
    def test_does_not_start_an_unverified_replacement(self):
        from unittest.mock import patch
        case = DeploymentTests()
        case.setUp()
        before = dict(case.machine, instance_id='version-1')
        before['config']['image'] = 'old-image'
        image = 'registry.fly.io/virtualcrypto-backup@sha256:' + 'a' * 64
        for drift in [{'image': 'wrong-image'}, {'schedule': 'hourly'},
                      {'env': {'UNEXPECTED': 'change'}}]:
            after = copy.deepcopy(before)
            after['config'].update(image=image)
            after['config'].update(drift)
            with self.subTest(drift=drift), \
                 patch.object(deploy, 'machines', side_effect=[[before], [after]]), \
                 patch.object(deploy, 'api', side_effect=[{'data': {'nonce': 'lease'}}, before, {}, {}]) as api, \
                 patch('sys.argv', ['update.py', image]), self.assertRaises(RuntimeError):
                deploy.main()
            self.assertEqual(api.call_count, 4)
            self.assertFalse(any(call.args[0].endswith('/start') for call in api.call_args_list))

    def test_does_not_retry_an_ambiguous_start(self):
        from unittest.mock import patch
        case = DeploymentTests()
        case.setUp()
        before = dict(case.machine, instance_id='version-1')
        before['config']['image'] = 'old-image'
        image = 'registry.fly.io/virtualcrypto-backup@sha256:' + 'a' * 64
        after = copy.deepcopy(before)
        after['config']['image'] = image
        with patch.object(deploy, 'machines', side_effect=[[before], [after]]), \
             patch.object(deploy, 'api', side_effect=[{'data': {'nonce': 'lease'}}, before, {}, {}, TimeoutError('start response lost')]) as api, \
             patch('sys.argv', ['update.py', image]), self.assertRaises(TimeoutError):
            deploy.main()
        self.assertEqual(api.call_count, 5)
        self.assertEqual(api.call_args_list[-1].args, ('existing/start',))

    def test_updates_only_image_with_version_guard_and_releases_lease(self):
        from unittest.mock import patch
        case = DeploymentTests()
        case.setUp()
        before = case.machine
        before['instance_id'] = 'version-1'
        before['config']['image'] = 'old-image'
        image = 'registry.fly.io/virtualcrypto-backup@sha256:' + 'a' * 64
        after = copy.deepcopy(before)
        after['config']['image'] = image
        transient = dict(after, state='replacing')
        with patch.object(deploy, 'time') as clock, \
             patch.object(deploy, 'machines', side_effect=[[before], [transient], [after]]), \
             patch.object(deploy, 'api', side_effect=[{'data': {'nonce': 'lease'}}, before, {}, {}, {}]) as api, \
             patch('sys.argv', ['update.py', image]):
            clock.monotonic.return_value = 0
            deploy.main()
            clock.sleep.assert_called_once_with(2)
        mutation = api.call_args_list[2]
        self.assertEqual(mutation.kwargs['data']['current_version'], 'version-1')
        self.assertTrue(mutation.kwargs['data']['skip_launch'])
        self.assertEqual(mutation.kwargs['nonce'], 'lease')
        expected = dict(before['config'], image=image)
        self.assertEqual(mutation.kwargs['data']['config'], expected)
        self.assertEqual(api.call_args_list[-2].kwargs['method'], 'DELETE')
        self.assertEqual(api.call_args_list[-1].args, ('existing/start',))
        self.assertEqual(api.call_args_list[-1].kwargs, {'method': 'POST'})

    def test_releases_lease_and_does_not_update_after_concurrent_change(self):
        from unittest.mock import patch
        case = DeploymentTests()
        case.setUp()
        before = dict(case.machine, instance_id='version-1')
        before['config']['image'] = 'old-image'
        current = dict(before, instance_id='version-2')
        image = 'registry.fly.io/virtualcrypto-backup@sha256:' + 'a' * 64
        with patch.object(deploy, 'machines', return_value=[before]), \
             patch.object(deploy, 'api', side_effect=[{'data': {'nonce': 'lease'}}, current, {}]) as api, \
             patch('sys.argv', ['update.py', image]), self.assertRaises(RuntimeError):
            deploy.main()
        self.assertEqual(api.call_count, 3)
        self.assertEqual(api.call_args_list[-1].kwargs['method'], 'DELETE')
