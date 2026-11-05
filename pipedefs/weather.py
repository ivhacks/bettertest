from bettertest import Stage, task
import subprocess

WORKER = "http://[::1]:7358"


class Weather(Stage):
    @task(WORKER)
    def wttr():
        subprocess.run(["curl", "-s", "wttr.in"], check=True)
