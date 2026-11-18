"""Run a command as a child and record its wall time and peak memory.

Usage: python3 measure.py OUT.json -- CMD [ARGS...]

The peak is getrusage(RUSAGE_CHILDREN).ru_maxrss: the kernel's exact peak
resident set size of the child, in KiB, read after the child is waited for.
SIGTERM to this wrapper is passed on to the child (SIGKILL after 20 s), so
`docker stop` on a server still writes OUT.json. The child's pid is written
to /tmp/child.pid for snapshots of /proc/<pid>/status.
"""
import json
import resource
import signal
import subprocess
import sys
import time

out = sys.argv[1]
cmd = sys.argv[sys.argv.index("--") + 1:]
t0 = time.time()
child = subprocess.Popen(cmd)
open("/tmp/child.pid", "w").write(str(child.pid))
stopped = []


def on_term(*_):
    stopped.append(time.time())
    child.terminate()


signal.signal(signal.SIGTERM, on_term)
while True:
    try:
        code = child.wait(timeout=1)
        break
    except subprocess.TimeoutExpired:
        if stopped and time.time() - stopped[0] > 20:
            child.kill()
wall = (stopped[0] if stopped else time.time()) - t0
ru = resource.getrusage(resource.RUSAGE_CHILDREN)
json.dump({"cmd": cmd, "exit": code, "stopped": bool(stopped), "wall_s": round(wall, 3),
           "maxrss_kib": ru.ru_maxrss, "user_s": round(ru.ru_utime, 2), "sys_s": round(ru.ru_stime, 2)},
          open(out, "w"), indent=1)
sys.exit(code if code >= 0 else 128 - code)
