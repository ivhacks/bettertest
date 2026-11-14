"""The pipeline narrow.py checks the pages with: many stages, long names, and a stage with too many
tasks to name. Every task runs on the worker of `bettertest unified`, without a sandbox."""

from bettertest import Stage, artifact, task, tests

WORKER = "http://[::1]:7358"


class ProvisionInfrastructure(Stage):
    worker = WORKER

    @task()
    def terraform():
        print("planned 12 resources")


class CompileEverythingFromSource(Stage):
    worker = WORKER

    @task()
    def compile_frontend_bundle():
        with open("bundle.js", "w") as bundle:
            bundle.write("x" * 4000)
        artifact("bundle.js", name="bundle-with-a-rather-long-name-for-downloading.js")

    @task()
    def compile_backend_services():
        print("a very long log line " * 30)


class UnitTestsAcrossAllSupportedPlatforms(Stage):
    worker = WORKER

    @task()
    def linux():
        tests(total=1520, failed=0)

    @task()
    def windows():
        tests(total=1490, failed=0)


class Deploy(Stage):
    worker = WORKER

    @task()
    def ship():
        print("shipped")


class EveryScriptOfTheWholeSuite(Stage):
    worker = WORKER


def script(n: int):
    def test() -> None:
        tests(total=n, failed=0)

    return test


for n in range(150):
    setattr(EveryScriptOfTheWholeSuite, f"script{n}", task()(script(n)))


class NotifyEveryoneOnTheMailingList(Stage):
    worker = WORKER

    @task()
    def email_the_whole_team():
        print("sent")
