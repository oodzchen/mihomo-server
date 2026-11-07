import { test, expect, type Page } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const source = await readFile(fileURLToPath(new URL("../src/theme.ts", import.meta.url)), "utf8");
const css = await readFile(fileURLToPath(new URL("../src/style.css", import.meta.url)), "utf8");
const script = `(() => { const exports = {}; ${ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText}; window.testTheme = exports; })();`;

async function mount(page: Page) {
  await page.route("http://theme.test/", route => route.fulfill({ contentType: "text/html", body: "<!doctype html><html><body><input value='unsaved'></body></html>" }));
  await page.goto("http://theme.test/");
  await page.addStyleTag({ content: css });
  await page.addScriptTag({ content: script });
}
const canvas = (page: Page) => page.locator("html").evaluate(node => getComputedStyle(node).backgroundColor);
const dark = "rgb(21, 21, 24)", light = "rgb(244, 244, 245)";

async function choose(page: Page, theme: string) {
  await page.evaluate(theme => (window as any).testTheme.applyTheme(theme), theme);
}
async function native(page: Page, theme: string, notify = true) {
  await page.evaluate(({ theme, notify }) => {
    (window as any).__MIHOMO_SYSTEM_THEME__ = { current: theme };
    if (notify) window.dispatchEvent(new Event("mihomo-system-theme-changed"));
  }, { theme, notify });
}

test("browser system theme tracks changes in both directions without changing the saved preference", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await mount(page);
  await expect.poll(() => canvas(page)).toBe(dark);
  await page.emulateMedia({ colorScheme: "light" });
  await expect.poll(() => canvas(page)).toBe(light);
  await page.emulateMedia({ colorScheme: "dark" });
  await expect.poll(() => canvas(page)).toBe(dark);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "system");
  expect(await page.evaluate(() => localStorage.getItem("mihomo-server-theme"))).toBe("system");
});

test("native desktop changes override a stale media query but never override manual choices", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await mount(page);
  const before = await page.locator("input").boundingBox();
  await native(page, "light");
  expect(await page.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches)).toBe(true);
  await expect.poll(() => canvas(page)).toBe(light);
  await native(page, "dark");
  await expect.poll(() => canvas(page)).toBe(dark);
  await choose(page, "light");
  await native(page, "dark");
  await expect.poll(() => canvas(page)).toBe(light);
  await choose(page, "dark");
  await native(page, "light");
  await expect.poll(() => canvas(page)).toBe(dark);
  await choose(page, "system");
  await expect.poll(() => canvas(page)).toBe(light);
  // A subsequent browser media event must not replace the authoritative native snapshot.
  await page.emulateMedia({ colorScheme: "light" });
  await page.emulateMedia({ colorScheme: "dark" });
  await expect.poll(() => canvas(page)).toBe(light);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "system");
  expect(await page.locator("input").boundingBox()).toEqual(before);
  await expect(page.locator("input")).toHaveValue("unsaved");
});

test("native changes cached before page startup are applied immediately", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.addInitScript(() => { (window as any).__MIHOMO_SYSTEM_THEME__ = { current: "light" }; });
  await mount(page);
  await expect.poll(() => canvas(page)).toBe(light);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "system");
});

for (const event of ["focus", "pageshow", "visibilitychange"]) {
  test(`system theme reconciles a missed notification on ${event}`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: "dark" });
    await mount(page);
    await native(page, "light", false);
    await expect.poll(() => canvas(page)).toBe(dark);
    await page.evaluate(event => (event === "visibilitychange" ? document : window).dispatchEvent(new Event(event)), event);
    await expect.poll(() => canvas(page)).toBe(light);
    await expect(page.locator("html")).toHaveAttribute("data-theme", "system");
  });
}

test("system theme works when browser storage is unavailable", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, "localStorage", { get() { throw new Error("storage unavailable"); } });
  });
  await page.emulateMedia({ colorScheme: "dark" });
  await mount(page);
  await native(page, "light");
  await expect.poll(() => canvas(page)).toBe(light);
  await choose(page, "dark");
  await expect.poll(() => canvas(page)).toBe(dark);
});
