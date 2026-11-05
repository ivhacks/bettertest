from bettertest import Stage, run, task

WORKER = "http://[::1]:7358"
# A `docker save` tarball uploaded to the boss. The worker loads it the first time it is used, so
# the worker never needs a registry or the internet.
ALPINE = "http://[::1]:7357/artifact/alpine.tar"


class FromArtifact(Stage):
    @task(WORKER, ALPINE)
    def cat_release():
        run("cat /etc/alpine-release")
