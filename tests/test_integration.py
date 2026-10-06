import os
import shutil
import subprocess
import sys
import time
import unittest
from unittest.mock import patch

from natbench.bench import benchmark, matrix
from natbench.lab import Lab


def namespaces():
    return set(subprocess.check_output(["ip", "netns", "list"], text=True).splitlines())


@unittest.skipUnless(os.geteuid() == 0 and shutil.which("ip") and shutil.which("nft"),
                     "requires root, iproute2 and nftables")
class Integration(unittest.TestCase):
    def test_matrix_and_cleanup(self):
        before = namespaces()
        results = matrix(timeout=1)
        self.assertEqual(len(results), 9)
        for result in results:
            self.assertTrue(result["relay"]["bidirectional_before"])
            self.assertTrue(all(result["relay"]["outage_detected"].values()))
            self.assertTrue(result["relay"]["bidirectional_after_restart"])
            profiles = result["profiles"]
            direct = result["traversal"]["bidirectional"]
            if set(profiles.values()) == {"preserve"}:
                self.assertTrue(direct, result)
                self.assertTrue(all(result["traversal"]["after_observer_shutdown"].values()))
            if "udp-blocked" in profiles.values() or set(profiles.values()) == {"random"}:
                self.assertFalse(direct, result)
            for side, profile in profiles.items():
                observation = result["observations"][side]
                if profile == "udp-blocked":
                    self.assertEqual(observation["mapping"], "unobserved")
                else:
                    self.assertEqual(observation["filtering"], "address-and-port-dependent")
                if profile == "preserve":
                    self.assertTrue(observation["port_preserved"])
        self.assertEqual(before, namespaces())

    def test_unsolicited_router_input_can_break_preserved_path(self):
        result = benchmark(router_input="accept", timeout=1)
        self.assertFalse(result["traversal"]["bidirectional"], result)

    def test_partial_setup_cleanup(self):
        before = namespaces()
        with patch.object(Lab, "_link", side_effect=RuntimeError("injected setup failure")):
            with self.assertRaisesRegex(RuntimeError, "injected"):
                with Lab():
                    pass
        self.assertEqual(before, namespaces())

    def test_user_exception_cleanup(self):
        before = namespaces()
        with self.assertRaisesRegex(RuntimeError, "injected"):
            with Lab() as lab:
                child = lab.spawn("a", "sleep", "60")
                raise RuntimeError("injected")
        self.assertIsNotNone(child.poll())
        self.assertEqual(before, namespaces())

    def test_cli_command_and_sigterm_cleanup(self):
        before = namespaces()
        result = subprocess.run([sys.executable, "-m", "natbench.cli", "run", "--", "false"])
        self.assertEqual(result.returncode, 1)
        process = subprocess.Popen([sys.executable, "-m", "natbench.cli", "run", "--", "sleep", "60"])
        try:
            deadline = time.monotonic() + 5
            while namespaces() == before and time.monotonic() < deadline:
                time.sleep(0.05)
            time.sleep(0.4)
            process.terminate()
            self.assertEqual(process.wait(timeout=8), 130)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
        self.assertEqual(before, namespaces())


if __name__ == "__main__":
    unittest.main()
