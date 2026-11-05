from bettertest import Labels, Stage, run, task


class Label(Labels):
    x86 = "x86"


# The cloud image as Debian publishes it. The worker downloads this URL.
DEBIAN = "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2"


class Debian(Stage):
    @task(vm=DEBIAN, labels=[Label.x86])
    def uname():
        run("uname -a")
        run("cat /etc/os-release")
        # Everything inside one task shares the vm, so state carries between run() calls.
        run("echo hello > /tmp/greeting")
        run("cat /tmp/greeting")
