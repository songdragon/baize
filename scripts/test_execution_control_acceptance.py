import unittest
from pathlib import Path
from execution_control_acceptance import fixture_command


class FixtureApprovalTests(unittest.TestCase):
    def permission(self, command, cwd="/fixture", method="item/commandExecution/requestApproval"):
        return {"native": {"method": method, "params": {"cwd": cwd, "command": command}}}

    def test_allows_only_the_exact_fixture_operation_in_its_workspace(self):
        for command in ["python3 worker.py approve", "python3 /fixture/worker.py approve", "/bin/zsh -lc 'python3 worker.py approve'"]:
            self.assertTrue(fixture_command(self.permission(command), Path("/fixture"), "approve"))

    def test_rejects_shell_injection_other_paths_modes_and_non_command_approvals(self):
        for command in ["python3 worker.py approve; touch other", "python3 worker.py approve && echo other", "python3 worker.py approve\nrm -rf other", "python3 other.py approve", "python3 worker.py deny", "python3 worker.py 'approve", "echo python3 worker.py approve"]:
            self.assertFalse(fixture_command(self.permission(command), Path("/fixture"), "approve"))
        self.assertFalse(fixture_command(self.permission("python3 worker.py approve", cwd="/elsewhere"), Path("/fixture"), "approve"))
        self.assertFalse(fixture_command(self.permission("python3 worker.py approve", method="item/fileChange/requestApproval"), Path("/fixture"), "approve"))
        self.assertFalse(fixture_command({}, Path("/fixture"), "approve"))
        permission=self.permission("python3 worker.py approve")
        permission["native"]["params"]["command"]=None
        self.assertFalse(fixture_command(permission,Path("/fixture"),"approve"))


if __name__ == "__main__":
    unittest.main()
