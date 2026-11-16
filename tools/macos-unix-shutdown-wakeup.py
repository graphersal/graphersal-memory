#!/usr/bin/env python3
"""A thread blocked in recv() on an accepted AF_UNIX stream socket is sometimes never woken
when the peer calls shutdown(SHUT_WR): the end of the stream is pending (a non-blocking
MSG_PEEK answers b"" at once) but the blocked recv() does not return.

Seen on macOS 27.0.1 (Darwin 27.0.0, arm64): 2 in 3,000 rounds in one process, 1 in 24,000
rounds in 8 processes (both while other tests ran). close() on the peer instead of shutdown(),
and a socketpair() instead of listen/accept, did not show it in 3,000 rounds. This is the
reproduction for Apple Feedback; graphersal-memory works around it in
`crates/graphersal-memory/src/runtime/ipc.rs`
(`Patient`: on macOS every read waits at most 1 s in the kernel, then reads again).

Usage: python3 -I tools/macos-unix-shutdown-wakeup.py [rounds] [processes]
(defaults: 5000 rounds in each of as many processes as CPUs; several processes at once make
the loss show sooner).
Exit code 0: no lost wake-up seen; 1: at least one seen. Standard library only; it uses a
temporary directory and nothing else.
"""

import os
import platform
import socket
import sys
import tempfile
import threading
import time

WAIT = 2.0  # seconds a reader may take to see the end of the stream before it counts as lost


def one(server, path, i, stuck):
    """One connect/accept/recv/shutdown round. Returns True when the wake-up was lost."""
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.connect(path)
    accepted, _ = server.accept()
    done = threading.Event()

    def reader():
        accepted.recv(16)  # blocks until the peer's shutdown ends the stream
        done.set()

    threading.Thread(target=reader, daemon=True).start()
    # The shutdown races with the reader entering recv(): at once on most rounds, after a short
    # pause on every third, so both orders occur.
    if i % 3 == 0:
        time.sleep(0.0005)
    client.shutdown(socket.SHUT_WR)
    if done.wait(WAIT):
        client.close()
        accepted.close()
        return False
    # The end of the stream is there: a non-blocking peek answers it at once.
    peek = accepted.recv(16, socket.MSG_PEEK | socket.MSG_DONTWAIT)
    print(f"  reader still blocked in recv() after {WAIT}s; MSG_PEEK answers {peek!r}"
          " (b'' = end of stream pending)", flush=True)
    stuck.append((client, accepted))  # left open: closing them would wake or break the reader
    return True


def worker(directory, number, iterations):
    """Runs `iterations` rounds on a listener of its own; returns the number of lost wake-ups."""
    path = os.path.join(directory, f"s{number}.sock")
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(path)
    server.listen(64)
    lost = 0
    stuck = []
    for i in range(1, iterations + 1):
        if one(server, path, i, stuck):
            lost += 1
            print(f"worker {number}, iteration {i}: lost wake-up", flush=True)
    server.close()
    return lost


def main():
    iterations = int(sys.argv[1]) if len(sys.argv) > 1 else 5000
    workers = int(sys.argv[2]) if len(sys.argv) > 2 else (os.cpu_count() or 4)
    print(f"{platform.platform()} ({platform.release()}), Python {platform.python_version()}; "
          f"{workers} processes x {iterations} rounds", flush=True)
    with tempfile.TemporaryDirectory() as directory:
        children = []
        for number in range(workers):
            pid = os.fork()  # before any thread exists in this process
            if pid == 0:
                os._exit(min(worker(directory, number, iterations), 255))
            children.append(pid)
        lost = 0
        for pid in children:
            _, status = os.waitpid(pid, 0)
            lost += os.waitstatus_to_exitcode(status) if hasattr(os, "waitstatus_to_exitcode") \
                else (status >> 8)
    print(f"done: {workers * iterations} rounds, {lost} lost wake-ups")
    return 1 if lost else 0


if __name__ == "__main__":
    sys.exit(main())
