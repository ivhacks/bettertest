from bettertest import Labels, Stage, task


class Label(Labels):
    x86 = "x86"


class Hello(Stage):
    labels = (Label.x86,)

    @task()
    def world():
        print("hello, world")
