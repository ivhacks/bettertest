from bettertest import Stage, run, task

WORKER = "http://[::1]:7358"
DEBIAN = "http://[::1]:7357/artifact/debian-13-genericcloud-amd64.qcow2"


class Debian(Stage):
    @task(WORKER, vm=DEBIAN)
    def uname():
        run("uname -a")
        run("cat /etc/os-release")
        # Everything inside one task shares the vm, so state carries between run() calls.
        run("echo hello > /tmp/greeting")
        run("cat /tmp/greeting")
