import subprocess

from bettertest import Labels, Stage, task


class Label(Labels):
    x86 = "x86"


class Weather(Stage):
    labels = (Label.x86,)

    @task()
    def wttr():
        subprocess.run(["curl", "-s", "wttr.in"], check=True)
