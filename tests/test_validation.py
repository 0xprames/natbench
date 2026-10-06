import unittest
from natbench.lab import Lab
from natbench.bench import benchmark


class Validation(unittest.TestCase):
    def test_invalid_profile(self):
        with self.assertRaises(ValueError):
            Lab(a="cone")

    def test_invalid_router_input(self):
        with self.assertRaises(ValueError):
            Lab(router_input="unknown")

    def test_invalid_timeout(self):
        for timeout in (0, -1, float("nan"), float("inf")):
            with self.subTest(timeout=timeout), self.assertRaises(ValueError):
                benchmark(timeout=timeout)
