import random
import time

from bettertest import Labels, Stage, task


class Label(Labels):
    x86 = "x86"


def take_a_while(name: str) -> None:
    delay = random.random() * 2
    print(f"{name} takes {delay:.1f}s")
    time.sleep(delay)


class Spread(Stage):
    # The tasks of a stage run at the same time (as many as the worker takes: worker --tasks).
    labels = (Label.x86,)

    @task()
    def first():
        take_a_while("first")

    @task()
    def second():
        take_a_while("second")

    @task()
    def third():
        take_a_while("third")


class Then(Stage):
    # A stage starts once every task of the one before it has passed.
    labels = (Label.x86,)

    @task()
    def last():
        take_a_while("last")
