"""Minimal TCP mailbox for fallback experiments, not a production relay."""
import json
import socket


def main():
    inbox = {"a": [], "b": []}
    with socket.socket() as server:
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server.bind(("198.18.0.1", 9100))
        server.listen()
        print(json.dumps({"ready": True}), flush=True)
        while True:
            connection, _ = server.accept()
            with connection, connection.makefile("r") as stream:
                connection.settimeout(2)
                try:
                    request = json.loads(stream.readline(4096))
                    if request["action"] == "put":
                        inbox[request["to"]].append(request["data"])
                        response = {"accepted": True}
                    elif request["action"] == "fetch":
                        response = inbox[request["role"]][:]
                        inbox[request["role"]].clear()
                    else:
                        response = {"error": "unknown action"}
                    connection.sendall((json.dumps(response) + "\n").encode())
                except (OSError, ValueError, KeyError):
                    pass


if __name__ == "__main__":
    main()
