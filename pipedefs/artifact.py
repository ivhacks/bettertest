from bettertest import Labels, Stage, run, task


class Label(Labels):
    x86 = "x86"


class Alpine(Stage):
    # A docker image name. The worker pulls it; the pipedef does not name a host.
    @task("alpine", labels=[Label.x86])
    def cat_release():
        run("cat /etc/alpine-release")
