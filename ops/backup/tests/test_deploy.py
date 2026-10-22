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
