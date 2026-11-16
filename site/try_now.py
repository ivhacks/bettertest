#!/usr/bin/env python3
"""The homepage Try now panel: the command follows the architecture, and the page does not
scroll sideways, including at 320px where the command wraps inside its box.
"""

from pathlib import Path

from playwright.sync_api import sync_playwright

HTML = Path(__file__).resolve().parent / "index.html"
X86 = 'curl -fsSL "https://real-live.bettertest.dev/install.sh?arch=x86_64" | sh'
ARM = 'curl -fsSL "https://real-live.bettertest.dev/install.sh?arch=aarch64" | sh'


def problems(page):
    return page.evaluate(
        """() => {
      const found = [];
      const doc = document.documentElement;
      if (doc.scrollWidth > doc.clientWidth + 1) {
        found.push(`page is ${doc.scrollWidth}px in ${doc.clientWidth}px`);
      }
      const cmd = document.querySelector("#try-cmd");
      const box = cmd.getBoundingClientRect();
      if (box.right > window.innerWidth + 1 || box.left < -1) {
        found.push(`command is outside the screen (${box.left}..${box.right} of ${window.innerWidth})`);
      }
      return found;
    }"""
    )


def main() -> None:
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch()
        page = browser.new_page()
        for width in (320, 1440):
            page.set_viewport_size({"width": width, "height": 800})
            page.set_content(HTML.read_text(), wait_until="domcontentloaded")
            assert page.locator("#try-panel").is_hidden()
            page.click("#try-now")
            page.wait_for_selector("#try-panel:not([hidden])")
            assert page.locator("#try-cmd").inner_text() == X86
            page.click('[data-arch="aarch64"]')
            assert page.locator("#try-cmd").inner_text() == ARM
            assert page.locator('[data-arch="aarch64"]').get_attribute("aria-pressed") == "true"
            page.evaluate(
                """() => {
                  window.__copied = null;
                  const clip = {
                    writeText: (text) => {
                      window.__copied = text;
                      return Promise.resolve();
                    },
                  };
                  if (navigator.clipboard) navigator.clipboard.writeText = clip.writeText;
                  else Object.defineProperty(navigator, "clipboard", { value: clip, configurable: true });
                }"""
            )
            page.click("#try-copy")
            assert page.evaluate("window.__copied") == ARM
            found = problems(page)
            assert found == [], f"at {width}px, arm command: {found}"
            page.click('[data-arch="x86_64"]')
            assert page.locator("#try-cmd").inner_text() == X86
            found = problems(page)
            assert found == [], f"at {width}px, x86 command: {found}"
        browser.close()


if __name__ == "__main__":
    main()
