// Wide layouts: no sideways page scroll, lanes without inner scrollbars, the floating detail in view.
import { test, expect, live } from "./fixtures";

for (const width of [1440, 1024, 800]) {
  test(`pages fit at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    for (const path of ["/", "/jobs/73", "/plans/100", "/repos", "/deploys", "/stats", "/configuration"]) {
      await page.goto(path);
      await expect(page.locator("h1, .job-card").first()).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth), path).toBeLessThanOrEqual(0);
    }
  });
}

test("swim lanes show no scrollbars of their own", async ({ page }) => {
  await live(page);
  // A lane may scroll when it is long, but no bar is drawn inside it.
  const bars = await page.locator(".lane").evaluateAll(lanes => lanes.map(l => l.offsetWidth - l.clientWidth - parseFloat(getComputedStyle(l).borderLeftWidth) - parseFloat(getComputedStyle(l).borderRightWidth)));
  expect(bars.every(width => width <= 0), JSON.stringify(bars)).toBe(true);
});

test("the floating detail sits below the header with its reply controls on screen", async ({ page }) => {
  await live(page, "/jobs/73");
  const header = await page.locator(".app-header").boundingBox();
  const meta = await page.locator(".detail-meta").boundingBox();
  expect(meta!.y).toBeGreaterThanOrEqual(header!.y + header!.height - 1);
  const viewport = page.viewportSize()!;
  for (const control of await page.locator(".reply-form input, .reply-form button").all()) {
    const box = (await control.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(viewport.width + 1);
    expect(box.y + box.height).toBeLessThanOrEqual(viewport.height + 1);
  }
  // The board behind stays usable: the composer is not covered by the panel.
  const composer = (await page.getByRole("textbox", { name: "New task", exact: true }).boundingBox())!;
  const panel = (await page.locator(".detail.open").boundingBox())!;
  expect(composer.x + composer.width <= panel.x || composer.y >= panel.y + panel.height, "the open panel covers New task").toBe(true);
});
