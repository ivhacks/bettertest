from bettertest import Stage, task

WORKER = "http://localhost:7358"


class Hello(Stage):
    @task(WORKER)
    def world():
        print("hello, world")
