"""Line-oriented UDP endpoint running inside a network namespace."""
import json
import select
import socket
import sys
import time


def main():
    sockets = []
    for endpoint in json.loads(sys.argv[1]):
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.bind(tuple(endpoint))
        sockets.append(sock)
    print(json.dumps({"ready": [s.getsockname() for s in sockets]}), flush=True)
    for line in sys.stdin:
        request = json.loads(line)
        if request["action"] == "send":
            sockets[request.get("socket", 0)].sendto(request["data"].encode(), tuple(request["to"]))
            result = {"sent": True}
        elif request["action"] == "relay":
            try:
                with socket.create_connection(("198.18.0.1", 9100), timeout=0.5) as conn:
                    conn.sendall((json.dumps(request["request"]) + "\n").encode())
                    with conn.makefile("r") as stream:
                        result = {"ok": True, "response": json.loads(stream.readline(4096))}
            except (OSError, ValueError) as exc:
                result = {"ok": False, "error": str(exc)}
        elif request["action"] == "receive":
            deadline = time.monotonic() + request.get("timeout", 0.5)
            result = None
            while time.monotonic() < deadline:
                ready, _, _ = select.select(sockets, [], [], max(0, deadline - time.monotonic()))
                if not ready:
                    break
                sock = ready[0]
                payload, source = sock.recvfrom(65535)
                data = payload.decode(errors="replace")
                if "match" not in request or data == request["match"]:
                    result = {"from": source, "data": data, "socket": sockets.index(sock)}
                    break
        else:
            raise ValueError("unknown action")
        print(json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
