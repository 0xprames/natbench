"""Measure mappings and filtering, then attempt simultaneous UDP traversal."""
import itertools
import json
import math
from pathlib import Path
import platform
import select
import subprocess
import sys
import time

from .lab import Lab, PROFILES

DESTINATIONS = (("198.18.0.1", 9000), ("198.18.0.1", 9001), ("198.18.0.2", 9000))


class Endpoint:
    def __init__(self, lab, role, binds):
        self.proc = lab.spawn(role, sys.executable, "-u", str(Path(__file__).with_name("worker.py")),
                              json.dumps(binds), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True)
        self.ready = self.read()

    def read(self, timeout=4):
        if not select.select([self.proc.stdout], [], [], timeout)[0]:
            raise RuntimeError("endpoint response timed out")
        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError("endpoint exited: " + self.proc.stderr.read())
        return json.loads(line)

    def request(self, action, **kwargs):
        self.proc.stdin.write(json.dumps({"action": action, **kwargs}) + "\n")
        self.proc.stdin.flush()
        return self.read(kwargs.get("timeout", 0.5) + 3)


def measure(client, observer, label):
    mappings = []
    for index, destination in enumerate(DESTINATIONS):
        token = f"mapping-{label}-{index}"
        client.request("send", to=destination, data=token)
        packet = observer.request("receive", match=token, timeout=0.3)
        mappings.append(packet["from"] if packet else None)
    if any(mapping is None for mapping in mappings):
        return {"observed_endpoints": mappings, "mapping": "unobserved",
                "filtering": "unobserved", "port_preserved": None}
    mapping = ("endpoint-independent" if mappings[0] == mappings[1] == mappings[2]
               else "address-dependent" if mappings[0] == mappings[1]
               else "address-and-port-dependent")
    # Use a fresh source socket that has contacted only destination 0.
    token = f"filter-open-{label}"
    client.request("send", socket=1, to=DESTINATIONS[0], data=token)
    packet = observer.request("receive", match=token, timeout=0.3)
    if packet is None:
        raise RuntimeError("filtering setup packet missing")
    target, allowed = packet["from"], []
    for index in (2, 1, 0):
        token = f"filter-{label}-{index}"
        observer.request("send", socket=index, to=target, data=token)
        received = client.request("receive", match=token, timeout=0.3)
        allowed.append(received is not None)
    filtering = ("endpoint-independent" if allowed[0] else
                 "address-dependent" if allowed[1] else
                 "address-and-port-dependent" if allowed[2] else "no-return-traffic")
    return {"observed_endpoints": mappings, "mapping": mapping, "filtering": filtering,
            "filter_accepts_different_ip_same_ip_original": allowed,
            "port_preserved": mappings[0][1] == client.ready["ready"][0][1]}


def start_relay(lab):
    proc = lab.spawn("wan", sys.executable, "-u", str(Path(__file__).with_name("relay.py")),
                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if not select.select([proc.stdout], [], [], 3)[0] or not proc.stdout.readline():
        raise RuntimeError("relay failed to start")
    return proc


def relay_exchange(clients, label):
    for side, peer in (("a", "b"), ("b", "a")):
        response = clients[side].request("relay", request={"action": "put", "to": peer,
                                                         "data": f"{label}-{side}"})
        if not response["ok"] or response["response"] != {"accepted": True}:
            return False
    for side, peer in (("a", "b"), ("b", "a")):
        response = clients[side].request("relay", request={"action": "fetch", "role": side})
        if not response["ok"] or response["response"] != [f"{label}-{peer}"]:
            return False
    return True


def benchmark(a="preserve", b="preserve", router_input="drop", timeout=2):
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("timeout must be finite and positive")
    started = time.monotonic()
    with Lab(a, b, router_input) as lab:
        observer = Endpoint(lab, "wan", DESTINATIONS)
        relay = start_relay(lab)
        clients = {side: Endpoint(lab, side, [("0.0.0.0", 10000), ("0.0.0.0", 10001)])
                   for side in ("a", "b")}
        fallback_before = relay_exchange(clients, "before")
        if not fallback_before:
            raise RuntimeError("TCP relay baseline failed")
        observations = {side: measure(client, observer, side) for side, client in clients.items()}
        targets = {side: observations[side]["observed_endpoints"][0] for side in clients}
        reached = {"a": False, "b": False}
        attempts = 0
        punch_started = time.monotonic()
        if all(targets.values()):
            while time.monotonic() - punch_started < timeout and not all(reached.values()):
                attempts += 1
                for side, peer in (("a", "b"), ("b", "a")):
                    clients[side].request("send", to=targets[peer], data=f"punch-{side}")
                for side, peer in (("a", "b"), ("b", "a")):
                    packet = clients[side].request("receive", match=f"punch-{peer}", timeout=0.1)
                    reached[side] |= packet is not None
        relay.terminate()
        relay.wait(timeout=3)
        outage = {side: not client.request("relay", request={"action": "fetch", "role": side})["ok"]
                  for side, client in clients.items()}
        observer.proc.terminate()
        observer.proc.wait(timeout=3)
        survived = {"a": False, "b": False}
        if all(reached.values()):
            for side, peer in (("a", "b"), ("b", "a")):
                clients[side].request("send", to=targets[peer], data=f"independent-{side}")
            for side, peer in (("a", "b"), ("b", "a")):
                survived[side] = clients[side].request("receive", match=f"independent-{peer}") is not None
        start_relay(lab)
        recovered = relay_exchange(clients, "recovered")
        return {"schema_version": 1, "kernel": platform.release(), "backend": "linux-nftables", "profiles": {"a": a, "b": b},
                "router_input": router_input, "observations": observations,
                "relay": {"bidirectional_before": fallback_before, "outage_detected": outage,
                          "bidirectional_after_restart": recovered},
                "traversal": {"received": reached, "bidirectional": all(reached.values()),
                              "after_observer_shutdown": survived, "attempts": attempts,
                              "timeout_seconds": timeout},
                "elapsed_seconds": round(time.monotonic() - started, 3)}


def matrix(router_input="drop", timeout=2):
    return [benchmark(a, b, router_input, timeout) for a, b in itertools.product(PROFILES, repeat=2)]
