import random
import subprocess
import time

from bettertest import Labels, Stage, task, tests, view


class Label(Labels):
    x86 = "x86"


class Quick(Stage):
    labels = (Label.x86,)

    @task()
    def alpha():
        time.sleep(0.1 + random.random() * 0.3)
        tests(total=100, failed=0)

    @task()
    def beta():
        time.sleep(0.2 + random.random() * 0.4)
        tests(total=150, failed=2)

    @task()
    def gamma():
        time.sleep(0.15 + random.random() * 0.2)
        tests(total=80, failed=1)


class Check(Stage):
    labels = (Label.x86,)

    @task(image="alpine")
    def packages():
        from bettertest import run

        time.sleep(0.3)
        run("apk list --installed | head -20")
        tests(total=50, failed=0)

    @task()
    def version():
        subprocess.run(["python3", "--version"], check=True)
        tests(total=10, failed=0)

    @task(image="mcr.microsoft.com/playwright:v1.49.1-noble")
    def browser():
        from bettertest import artifact, run

        run("pip install --quiet pytest-playwright")
        run(
            """cat > test_sample.py << 'EOF'
import pytest
from playwright.sync_api import Page, expect

def test_example(page: Page):
    page.goto("https://example.com")
    expect(page).to_have_title("Example Domain")
    page.screenshot(path="screenshot.png")
EOF"""
        )
        run("pytest --html=report.html --self-contained-html test_sample.py")
        artifact("/work/report.html", name="playwright-report.html")
        tests(total=1, failed=0)


@view
def outcomes(results):
    import plotly.express as px

    task_results = []
    for stage in results:
        for task in stage["tasks"]:
            task_results.append(
                {
                    "task": f"{stage['name']}.{task['name']}",
                    "status": task["state"],
                }
            )
    status_counts = {}
    for r in task_results:
        status_counts[r["status"]] = status_counts.get(r["status"], 0) + 1
    fig = px.pie(
        names=list(status_counts.keys()),
        values=list(status_counts.values()),
        title="Task Outcomes",
    )
    return fig


@view
def durations(results):
    import plotly.express as px

    task_data = []
    for stage in results:
        for task in stage["tasks"]:
            if task.get("started") and task.get("ended"):
                duration_ms = task["ended"] - task["started"]
                task_data.append(
                    {
                        "task": f"{stage['name']}.{task['name']}",
                        "duration": duration_ms / 1000.0,
                    }
                )
    if not task_data:
        task_data = [{"task": "no data", "duration": 0}]
    fig = px.bar(
        task_data,
        x="task",
        y="duration",
        title="Task Durations (seconds)",
        labels={"duration": "Duration (s)"},
    )
    return fig


@view
def test_summary(results):
    import plotly.graph_objects as go

    total_tests = 0
    total_failed = 0
    for stage in results:
        for task in stage["tasks"]:
            if task.get("tests"):
                total_tests += task["tests"]["total"]
                total_failed += task["tests"]["failed"]
    fig = go.Figure()
    fig.add_trace(
        go.Indicator(
            mode="number+delta",
            value=total_tests - total_failed,
            title={"text": "Passing Tests"},
            delta={"reference": total_tests, "relative": False},
        )
    )
    fig.update_layout(height=300)
    return fig


@view
def logs_side_by_side(results):
    import plotly.graph_objects as go

    logs = {}
    for stage in results:
        for task in stage["tasks"]:
            task_name = f"{stage['name']}.{task['name']}"
            logs[task_name] = f"Log placeholder for {task_name}"
    if len(logs) < 2:
        logs = {"Task 1": "Sample log 1", "Task 2": "Sample log 2"}
    task_names = list(logs.keys())[:2]
    fig = go.Figure()
    fig.add_annotation(
        text=f"<b>{task_names[0]}</b><br>{logs[task_names[0]]}",
        xref="paper",
        yref="paper",
        x=0.25,
        y=0.5,
        showarrow=False,
        font=dict(family="monospace", size=12),
    )
    fig.add_annotation(
        text=f"<b>{task_names[1]}</b><br>{logs[task_names[1]]}",
        xref="paper",
        yref="paper",
        x=0.75,
        y=0.5,
        showarrow=False,
        font=dict(family="monospace", size=12),
    )
    fig.update_layout(
        title="Side-by-Side Logs",
        showlegend=False,
        xaxis=dict(showgrid=False, showticklabels=False, zeroline=False),
        yaxis=dict(showgrid=False, showticklabels=False, zeroline=False),
        height=400,
    )
    return fig
