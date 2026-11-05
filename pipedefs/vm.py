from bettertest import Labels, Stage, run, task


class Label(Labels):
    x86 = "x86"


DEBIAN = "http://[::1]:7357/artifact/debian-13-genericcloud-amd64.qcow2"


class Debian(Stage):
    @task(vm=DEBIAN, labels=[Label.x86])
    def uname():
        run("uname -a")
        run("cat /etc/os-release")
        # Everything inside one task shares the vm, so state carries between run() calls.
        run("echo hello > /tmp/greeting")
        run("cat /tmp/greeting")
