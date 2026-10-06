import argparse
import json
import math
import signal
import subprocess
import sys

from .bench import benchmark, matrix
from .lab import Lab, PROFILES


def interrupted(signum, frame):
    raise KeyboardInterrupt


def main():
    parser = argparse.ArgumentParser(description="Measure NAT behavior in isolated Linux namespaces")
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("bench", "matrix", "run"):
        p = sub.add_parser(name)
        p.add_argument("--router-input", choices=("drop", "accept"), default="drop")
        if name != "matrix":
            p.add_argument("--a", choices=PROFILES, default="preserve")
            p.add_argument("--b", choices=PROFILES, default="preserve")
        if name == "run":
            p.add_argument("--role", choices=("a", "b", "ra", "rb", "wan"), default="a")
            p.add_argument("argv", nargs=argparse.REMAINDER)
        else:
            p.add_argument("--timeout", type=float, default=2)
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, interrupted)
    try:
        if args.command == "run":
            argv = args.argv[1:] if args.argv[:1] == ["--"] else args.argv
            if not argv:
                parser.error("run requires a command after --")
            with Lab(args.a, args.b, args.router_input) as lab:
                return lab.spawn(args.role, *argv).wait()
        if not math.isfinite(args.timeout) or args.timeout <= 0:
            parser.error("--timeout must be finite and positive")
        result = (matrix(args.router_input, args.timeout) if args.command == "matrix" else
                  benchmark(args.a, args.b, args.router_input, args.timeout))
        print(json.dumps(result, indent=2))
        return 0
    except KeyboardInterrupt:
        return 130
    except subprocess.CalledProcessError as exc:
        print(f"natbench: {exc}: {exc.stderr}", file=sys.stderr)
        return 1
    except (OSError, RuntimeError) as exc:
        print(f"natbench: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
