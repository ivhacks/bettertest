from bettertest import Stage, run, task

WORKER = "http://localhost:9009"
IMAGE = "debian:bookworm"


class Suck(Stage):
    @task(WORKER, IMAGE)
    def gurt():
        run("echo suck gurt")


class Slurp(Stage):
    @task(WORKER, IMAGE)
    def gurt():
        run("echo slurp gurt")


class Slobber(Stage):
    @task(WORKER, IMAGE)
    def gurt():
        run("echo slobber gurt")

    @task(WORKER, IMAGE)
    def yo():
        run("echo slobber yo")
