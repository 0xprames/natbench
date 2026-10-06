"""Kernel-backed network fixture without application protocol assumptions."""
import os
import secrets
import shutil
import subprocess
import time

PROFILES = ("preserve", "random", "udp-blocked")
PUBLIC = {"a": "198.18.0.10", "b": "198.18.0.20"}


def command(*argv):
    return subprocess.run(argv, check=True, text=True, capture_output=True)


class Lab:
    def __init__(self, a="preserve", b="preserve", router_input="drop"):
        if a not in PROFILES or b not in PROFILES:
            raise ValueError(f"profiles must be one of {PROFILES}")
        if router_input not in ("drop", "accept"):
            raise ValueError("router_input must be drop or accept")
        self.profiles = {"a": a, "b": b}
        self.router_input = router_input
        self.tag = "nb" + secrets.token_hex(4)
        self.namespaces = {r: f"{self.tag}-{r}" for r in ("wan", "a", "b", "ra", "rb")}
        self.created, self.processes, self.links = [], [], []

    def run(self, role, *argv, **kwargs):
        return subprocess.run(["ip", "netns", "exec", self.namespaces[role], *map(str, argv)],
                              check=True, text=True, capture_output=True, **kwargs)

    def spawn(self, role, *argv, **kwargs):
        proc = subprocess.Popen(["ip", "netns", "exec", self.namespaces[role], *map(str, argv)], **kwargs)
        self.processes.append(proc)
        return proc

    def _link(self, left, left_if, right, right_if):
        x, y = "v" + secrets.token_hex(5), "v" + secrets.token_hex(5)
        command("ip", "link", "add", x, "type", "veth", "peer", "name", y)
        self.links.extend((x, y))
        for name, role, device in ((x, left, left_if), (y, right, right_if)):
            command("ip", "link", "set", name, "netns", self.namespaces[role], "name", device)
            self.run(role, "ip", "link", "set", "dev", device, "up")

    def __enter__(self):
        if os.geteuid() != 0:
            raise RuntimeError("network namespaces require root")
        for tool in ("ip", "nft", "sysctl"):
            if not shutil.which(tool):
                raise RuntimeError(f"missing executable: {tool}")
        try:
            for role, name in self.namespaces.items():
                command("ip", "netns", "add", name)
                self.created.append(name)
                self.run(role, "ip", "link", "set", "lo", "up")
            self.run("wan", "ip", "link", "add", "br0", "type", "bridge")
            self.run("wan", "ip", "link", "set", "br0", "up")
            for address in ("198.18.0.1", "198.18.0.2"):
                self.run("wan", "ip", "addr", "add", address + "/24", "dev", "br0")
            for index, side in enumerate(("a", "b"), 1):
                router = "r" + side
                self._link(side, "eth0", router, "lan")
                self._link(router, "wan", "wan", side)
                self.run("wan", "ip", "link", "set", "dev", side, "master", "br0")
                for role, device, address in ((side, "eth0", f"10.{index}.0.2/24"),
                                               (router, "lan", f"10.{index}.0.1/24"),
                                               (router, "wan", PUBLIC[side] + "/24")):
                    self.run(role, "ip", "addr", "add", address, "dev", device)
                self.run(side, "ip", "route", "add", "default", "via", f"10.{index}.0.1")
                self.run(router, "sysctl", "-q", "-w", "net.ipv4.ip_forward=1")
                random = " fully-random" if self.profiles[side] == "random" else ""
                blocked = 'meta l4proto udp drop;' if self.profiles[side] == "udp-blocked" else ""
                rules = f'''table ip translation {{
 chain outbound {{ type nat hook postrouting priority srcnat; oifname "wan" masquerade{random}; }}
}}
table ip firewall {{
 chain router {{ type filter hook input priority filter; policy {self.router_input};
 ct state established,related accept; iifname {{ "lo", "lan" }} accept;
 }}
 chain transit {{ type filter hook forward priority filter; policy drop;
 {blocked}
 ct state established,related accept; iifname "lan" oifname "wan" accept;
 }}
}}
'''
                self.run(router, "nft", "-f", "-", input=rules)
            return self
        except BaseException:
            self.close()
            raise

    def close(self):
        for proc in self.processes:
            if proc.poll() is None:
                proc.terminate()
        deadline = time.monotonic() + 3
        for proc in self.processes:
            if proc.poll() is None:
                try:
                    proc.wait(timeout=max(0.01, deadline - time.monotonic()))
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
        for proc in self.processes:
            for stream in (proc.stdin, proc.stdout, proc.stderr):
                if stream is not None:
                    stream.close()
        self.processes.clear()
        for name in reversed(self.created):
            result = subprocess.run(["ip", "netns", "pids", name], capture_output=True, text=True)
            for pid in result.stdout.split():
                try:
                    os.kill(int(pid), 9)
                except ProcessLookupError:
                    pass
            subprocess.run(["ip", "netns", "del", name], capture_output=True)
        self.created.clear()
        for name in self.links:
            subprocess.run(["ip", "link", "del", name], capture_output=True)
        self.links.clear()

    def __exit__(self, *exc):
        self.close()
