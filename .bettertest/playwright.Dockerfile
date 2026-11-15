# Playwright's image holds its browsers but not its Python package; both at one version. The
# frontend's browser tests (frontend/test/shell_nav.rs) and the pipeline's page timings run in it.
FROM mcr.microsoft.com/playwright/python:v1.65.1-noble
RUN pip install --break-system-packages --no-cache-dir playwright==1.65.1
