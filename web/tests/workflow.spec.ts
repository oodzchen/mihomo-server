import { test, expect } from "@playwright/test";
import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { createHash } from "node:crypto";
import { gzipSync } from "node:zlib";
import { createServer } from "node:net";
import { createServer as createHttpServer, type Server } from "node:http";
import { createServer as createHttpsServer } from "node:https";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";

let service: ChildProcess, directory: string, base: string, token: string;
let subscription: Server, subscriptionUrl: string;
let subscriptionBody = "proxies: []\nmode: direct\n";
let subscriptionUsage = "upload=1024; download=2048; total=4096; expire=0";
let subscriptionRequests = 0;
let tlsSubscription: Server, tlsSubscriptionUrl: string;
let tlsSubscriptionRequests = 0;
const errors: string[] = [];
// Existing workflows deliberately expand the advanced groups they exercise.
async function openSettingsForEditing(page: import("@playwright/test").Page) {
  if (new URL(page.url()).pathname !== "/settings") return;
  await expect(page.locator(".settings-group").first().or(page.locator(".settings-layout [role=alert]").first()).first()).toBeAttached();
  await expect(page.locator(".settings-reload button")).toBeEnabled();
  await page.locator(".settings-group").evaluateAll(nodes => nodes.forEach(node => (node as HTMLDetailsElement).open = true));
}

async function settingsApi(command: string, fields: Record<string, unknown> = {}) {
  const response = await fetch(`${base}/api/commands`, {
    method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    body: JSON.stringify({ command, ...fields }),
  });
  expect(response.ok).toBe(true);
  return response.json();
}
async function applySettings(page: import("@playwright/test").Page) {
  const save = page.getByRole("button", { name: "保存服务设置", exact: true });
  // Let the settings page publish its derived dirty state to the shared header.
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  if (await save.isDisabled()) return;
  await save.click();
  await expect.poll(async () =>
    (await save.isDisabled()) || (await page.getByRole("region", { name: "服务设置编辑器" }).getByRole("alert").count()) > 0,
  ).toBeTruthy();
  await expect(page.locator(".settings-reload button")).toBeEnabled();
  await expect(page.locator(".toast-loading")).toHaveCount(0);
}
async function editSetting(page: import("@playwright/test").Page, name: string, value: string, select = false) {
  const control = page.getByRole(select ? "combobox" : "textbox", { name, exact: true });
  await expect(page.locator(".settings-reload button")).toBeEnabled();
  await control.focus();
  if (select) await control.selectOption(value); else await control.fill(value);
  await applySettings(page);
}
async function loginSettings(page: import("@playwright/test").Page) {
  await page.goto(`${base}/settings#token=${encodeURIComponent(token)}`);
  await openSettingsForEditing(page);
}
// Long explanations live in hover help, not in the page flow.
async function expectHelp(page: import("@playwright/test").Page, label: string, text: string) {
  await expect(page.getByText(text, { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: label, exact: true }).hover();
  await expect(page.getByRole("tooltip")).toContainText(text);
  await page.mouse.move(0, 0);
  await expect(page.getByRole("tooltip")).toHaveCount(0);
}

async function start() {
  const fixtureEnv = { ...process.env, MIHOMO_SERVER_DATA_DIR: directory };
  for (const name of [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
    "REQUEST_METHOD",
  ])
    delete fixtureEnv[name as keyof typeof fixtureEnv];
  Object.assign(fixtureEnv, {
    HTTP_PROXY: subscriptionUrl,
    NO_PROXY: "127.0.0.1,localhost",
  });
  const bundle = process.env.MIHOMO_TEST_BUNDLE;
  service = spawn(
    bundle
      ? join(resolve(bundle), "launch")
      : process.env.MIHOMO_SERVER_BINARY || resolve("../target/debug/mihomo-server"),
    bundle
      ? [
          "--listen",
          new URL(base).host,
          "--config",
          join(directory, "missing.yaml"),
        ]
      : [
          "--mihomo",
          process.env.MIHOMO_TEST_BINARY || "/usr/bin/verge-mihomo",
          "--data-dir",
          directory,
          "--listen",
          new URL(base).host,
          "--web-dir",
          process.env.MIHOMO_TEST_WEB_DIR || resolve("dist"),
        ],
    {
      stdio: ["ignore", "ignore", "pipe"],
      env: fixtureEnv,
    },
  );
  let stderr = "";
  service.stderr!.on("data", (chunk) => {
    stderr += chunk.toString();
  });
  await expect
    .poll(async () => {
      if (service.exitCode !== null)
        throw new Error(`Service exited: ${stderr}`);
      try {
        token = (
          await readFile(join(directory, "management-token"), "utf8")
        ).trim();
        return (
          await fetch(`${base}/api/status`, {
            headers: { Authorization: `Bearer ${token}` },
          })
        ).status;
      } catch {
        return 0;
      }
    })
    .toBe(200);
}
async function stop() {
  if (service.exitCode !== null) return;
  const exit = new Promise<number | null>((resolve) =>
    service.once("exit", resolve),
  );
  service.kill("SIGTERM");
  const deadline = setTimeout(() => service.kill("SIGKILL"), 10000);
  const code = await exit;
  clearTimeout(deadline);
  expect(code).toBe(0);
}
test.beforeAll(async () => {
  subscription = createHttpServer((request, response) => {
    subscriptionRequests += 1;
    if (request.headers.authorization)
      throw new Error("Management token forwarded to provider");
    if (request.url === "/error") {
      response.writeHead(503);
      response.end("provider unavailable");
    } else {
      response.writeHead(200, {
        "Content-Disposition": "attachment; filename=BrowserRemote.yaml",
        "Subscription-Userinfo": subscriptionUsage,
      });
      response.end(subscriptionBody);
    }
  });
  await new Promise<void>((resolve) =>
    subscription.listen(0, "127.0.0.1", resolve),
  );
  const provider = subscription.address();
  if (!provider || typeof provider === "string")
    throw new Error("No provider address");
  subscriptionUrl = `http://127.0.0.1:${provider.port}`;
  directory = await mkdtemp(join(tmpdir(), "ms-browser-"));
  execFileSync(
    "openssl",
    [
      "req",
      "-x509",
      "-newkey",
      "rsa:2048",
      "-nodes",
      "-days",
      "1",
      "-subj",
      "/CN=fixture.invalid",
      "-addext",
      "subjectAltName=DNS:fixture.invalid",
      "-out",
      join(directory, "tls.pem"),
      "-keyout",
      join(directory, "tls.key"),
    ],
    { stdio: "ignore" },
  );
  tlsSubscription = createHttpsServer(
    {
      cert: await readFile(join(directory, "tls.pem")),
      key: await readFile(join(directory, "tls.key")),
    },
    (request, response) => {
      tlsSubscriptionRequests += 1;
      if (
        request.headers.authorization ||
        request.headers["proxy-authorization"]
      )
        throw new Error("Authentication forwarded to HTTPS provider");
      response.writeHead(200, { "Subscription-Userinfo": subscriptionUsage });
      response.end("proxies: []\nmode: direct\n");
    },
  );
  tlsSubscription.on("tlsClientError", () => {}); // Expected strict-verifier failures.
  await new Promise<void>((resolve) =>
    tlsSubscription.listen(0, "127.0.0.1", resolve),
  );
  const tlsProvider = tlsSubscription.address();
  if (!tlsProvider || typeof tlsProvider === "string")
    throw new Error("No TLS provider address");
  tlsSubscriptionUrl = `https://127.0.0.1:${tlsProvider.port}/subscription?token=private-browser-token`;
  const port = Math.floor(Math.random() * 20000 + 30000);
  base = `http://127.0.0.1:${port}`;
  await start();
});
// The interface language is shared through the service; start every test unset.
test.afterEach(async () => {
  // Tolerant: a test may leave the service stopped.
  await fetch(`${base}/api/commands`, {
    method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    body: JSON.stringify({ command: "set_language", language: null }),
  }).catch(() => undefined);
});

test.afterAll(async () => {
  try {
    await stop();
  } finally {
    await rm(directory, { recursive: true, force: true });
    await new Promise<void>((resolve) => subscription.close(() => resolve()));
    await new Promise<void>((resolve) =>
      tlsSubscription.close(() => resolve()),
    );
  }
});

for (const parameter of ["fragment", "query"]) {
  test(`management link ${parameter} token logs in automatically and is consumed`, async ({ page }) => {
    // A new link must override a stale credential left in this tab.
    await page.goto(base);
    await expect(page.getByLabel("管理令牌")).toBeVisible();
    await page.evaluate(() => sessionStorage.setItem("mihomo.token", "stale-token"));
    const requests: string[] = [];
    page.on("request", request => requests.push(request.url()));
    const link = parameter === "fragment"
      ? `${base}/?view=install#panel=overview&token=${token}`
      : `${base}/?view=install&token=${token}#panel=overview`;
    await page.goto(link);
    await expect(page.getByRole("heading", { name: "概览", exact: true })).toBeVisible();
    await expect(page).toHaveURL(`${base}/?view=install#panel=overview`);
    expect(await page.evaluate(() => sessionStorage.getItem("mihomo.token"))).toBe(token);
    expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toContain(token);
    if (parameter === "fragment") expect(requests.some(url => url.includes(token))).toBe(false);
    expect(requests.filter(url => new URL(url).pathname.startsWith("/api/"))
      .some(url => url.includes(token))).toBe(false);
    await page.reload();
    await expect(page.getByRole("heading", { name: "概览", exact: true })).toBeVisible();
    await page.getByRole("button", { name: "退出登录" }).click();
    await expect(page.getByLabel("管理令牌")).toHaveValue("");
    expect(await page.evaluate(() => sessionStorage.getItem("mihomo.token"))).toBeNull();
    await page.reload();
    await expect(page.getByRole("heading", { name: "连接你的服务" })).toBeVisible();
  });
}

test("invalid management link token is removed and permits manual login", async ({ page }) => {
  await page.goto(`${base}/#token=incorrect`);
  await expect(page.getByRole("alert")).toContainText("令牌无效");
  await expect(page).toHaveURL(`${base}/`);
  expect(await page.evaluate(() => sessionStorage.getItem("mihomo.token"))).toBeNull();
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await expect(page.getByRole("heading", { name: "概览", exact: true })).toBeVisible();
  await page.reload();
  await expect(page.getByRole("heading", { name: "概览", exact: true })).toBeVisible();
});

test("failed management link verification shows the login error without caching the token", async ({ page }) => {
  await page.route("**/api/commands", route => route.fulfill({
    status: 503, contentType: "application/json",
    body: JSON.stringify({ error: { message: "temporarily unavailable" } }),
  }));
  await page.goto(`${base}/#token=${token}`);
  await expect(page.getByRole("alert")).toContainText("temporarily unavailable");
  await expect(page).toHaveURL(`${base}/`);
  expect(await page.evaluate(() => sessionStorage.getItem("mihomo.token"))).toBeNull();
});

test("browser language selection persists locally without changing service state", async ({ page, browser }) => {
  await page.goto(base);
  await expect(page.getByRole("heading", { name: "连接你的服务" })).toBeVisible();
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("heading", { name: "Connect to your service" })).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await page.reload();
  await expect(page.getByRole("heading", { name: "Connect to your service" })).toBeVisible();
  await page.getByLabel("Management token").fill(token);
  await page.getByRole("button", { name: "Connect to service" }).click();
  const navigation = page.getByRole("navigation", { name: "Main navigation" });
  await expect(navigation.getByRole("link", { name: "Overview" })).toBeVisible();
  await navigation.getByRole("link", { name: "Core" }).click();
  await expect(page.getByRole("button", { name: "Start core", exact: true })).toBeVisible();
  const before = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: "概览" })).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");
  const after = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  expect(after.generation).toBe(before.generation);
  const storage = await page.evaluate(() => Object.fromEntries(Object.entries(localStorage)));
  expect(storage["mihomo-server-language"]).toBe("zh");
  expect(JSON.stringify(storage)).not.toContain(token);

  const separate = await browser.newContext();
  try {
    const otherPage = await separate.newPage();
    await otherPage.goto(base);
    await expect(otherPage.getByRole("heading", { name: "连接你的服务" })).toBeVisible();
    await expect(otherPage.getByRole("combobox", { name: "界面语言" })).toHaveValue("zh");
  } finally {
    await separate.close();
  }
});

test("browser language selection supports traditional chinese zhtw and persists locally", async ({ page, browser }) => {
  await page.goto(base);
  await expect(page.getByRole("heading", { name: "连接你的服务" })).toBeVisible();
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("zhtw");
  await expect(page.getByRole("heading", { name: "連線你的服務" })).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("lang", "zh-TW");
  await page.reload();
  await expect(page.getByRole("heading", { name: "連線你的服務" })).toBeVisible();
  await page.getByLabel("管理權杖").fill(token);
  await page.getByRole("button", { name: "連線服務" }).click();
  const navigation = page.getByRole("navigation", { name: "主導航" });
  await expect(navigation.getByRole("link", { name: "概覽" })).toBeVisible();
  await expect(navigation.getByRole("link", { name: "規則" })).toBeVisible();
  await expect(navigation.getByRole("link", { name: "記錄" })).toBeVisible();
  await expect(navigation.getByRole("link", { name: "核心" })).toBeVisible();
  await navigation.getByRole("link", { name: "核心" }).click();
  await expect(page.getByRole("button", { name: "啟動核心", exact: true })).toBeVisible();

  const storage = await page.evaluate(() => Object.fromEntries(Object.entries(localStorage)));
  expect(storage["mihomo-server-language"]).toBe("zhtw");
  expect(JSON.stringify(storage)).not.toContain(token);

  await page.getByRole("combobox", { name: "介面語言" }).selectOption("en");
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");
});

test("configuration editor translates without losing an unapplied YAML draft", async ({ page }) => {
  await page.goto(`${base}/config`);
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await page.getByLabel("Management token").fill(token);
  await page.getByRole("button", { name: "Connect to service" }).click();
  await expect(page.getByRole("heading", { name: "Runtime configuration" })).toBeVisible();
  await expect(page.getByText("No committed configuration yet. Paste YAML to apply it.")).toBeVisible();
  const editor = page.getByLabel("Runtime configuration YAML");
  await editor.fill("mode: rule\n");
  await expect(page.getByText("Unapplied changes")).toBeVisible();
  await expect(page.getByRole("button", { name: "Validate and apply" })).toBeEnabled();
  const before = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());

  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.getByRole("heading", { name: "运行配置" })).toBeVisible();
  await expect(page.getByText("尚无已提交配置，可粘贴 YAML 后应用。")).toBeVisible();
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue("mode: rule\n");
  await expect(page.getByText("有未应用的修改")).toBeVisible();
  const after = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  expect(after.generation).toBe(before.generation);
});

test("profile list switches language while keeping deletion confirmation", async ({ page }) => {
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await expect(page.getByRole("heading", { name: "订阅列表" })).toBeVisible();
  await expect(page.getByText("还没有订阅。导入一个 YAML 文件开始使用。")).toBeVisible();
  await page.getByRole("button", { name: "+ 本地订阅" }).first().click();
  await page.getByLabel("订阅名称", { exact: true }).fill("LanguageFixture");
  await page.getByLabel("订阅 YAML", { exact: true }).fill("proxies: []\nmode: direct\n");
  await page.getByRole("button", { name: "导入订阅", exact: true }).click();
  const item = page.locator("article.profile").filter({ has: page.getByRole("heading", { name: "LanguageFixture" }) });
  await expect(item).toContainText("本地订阅");
  const before = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());

  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("heading", { name: "Profile list" })).toBeVisible();
  await expect(page.getByText("1 profile", { exact: true })).toBeVisible();
  await expect(item).toContainText("Local profile");
  await expect(item.getByRole("button", { name: "Use profile" })).toBeVisible();
  await item.getByRole("button", { name: "Actions LanguageFixture" }).click();
  await item.getByRole("button", { name: "Delete profile LanguageFixture" }).click();
  await expect(item.getByText("Delete this profile, its exclusive auxiliary configurations and DNS preferences? Shared auxiliary configurations will remain.")).toBeVisible();

  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(item.getByText("删除此订阅、独占的辅助配置和 DNS 偏好？共享辅助配置会保留。")).toBeVisible();
  await item.getByRole("button", { name: "确认删除 LanguageFixture" }).click();
  await expect(page.getByText("还没有订阅。导入一个 YAML 文件开始使用。")).toBeVisible();
  const after = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  expect(after.generation).toBe(before.generation);
});

test("subscription import forms translate without losing drafts or changing imports", async ({ page }) => {
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");

  // Open local modal in en
  await page.getByRole("button", { name: "+ Local profile" }).first().click();
  await expect(page.getByRole("heading", { name: "Import local profile" })).toBeVisible();
  await page.getByLabel("Upload profile YAML").setInputFiles({
    name: "large.yaml", mimeType: "text/yaml", buffer: Buffer.alloc(8 * 1024 ** 2 + 1),
  });
  await expect(page.getByText("File must not exceed 8 MiB")).toBeVisible();
  await page.getByRole("button", { name: "Close" }).click();

  // Open remote modal in en
  await page.getByRole("button", { name: "+ Remote profile" }).first().click();
  await expect(page.getByRole("heading", { name: "Download remote profile" })).toBeVisible();
  await page.getByLabel("Subscription URL").fill(`${subscriptionUrl}/ok`);
  await page.getByLabel("Remote profile name (optional)").fill("RemoteDraft");
  await page.getByLabel("Allow invalid TLS certificates for downloads").check();
  const before = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());

  // Switch language to zh
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.getByLabel("订阅链接")).toHaveValue(`${subscriptionUrl}/ok`);
  await expect(page.getByLabel("远程订阅名称（可选）")).toHaveValue("RemoteDraft");
  await expect(page.getByLabel("下载允许无效 TLS 证书")).toBeChecked();
  await page.getByRole("button", { name: "关闭" }).click();

  // Open local modal in zh
  await page.getByRole("button", { name: "+ 本地订阅" }).first().click();
  await expect(page.getByText("文件不能超过 8 MiB")).toBeVisible();
  await page.getByLabel("上传订阅 YAML").setInputFiles({
    name: "LocalDraft.yaml", mimeType: "text/yaml", buffer: Buffer.from("proxies: []\nmode: direct\n"),
  });
  await expect(page.getByLabel("订阅名称", { exact: true })).toHaveValue("LocalDraft");
  await expect(page.getByText("文件不能超过 8 MiB")).toHaveCount(0);
  const after = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  expect(after.generation).toBe(before.generation);
  await page.getByRole("button", { name: "关闭" }).click();

  // Switch to en and import both
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await page.getByRole("button", { name: "+ Local profile" }).first().click();
  await page.getByRole("button", { name: "Import profile", exact: true }).click();
  await expect(page.getByRole("heading", { name: "LocalDraft" })).toBeVisible();

  await page.getByRole("button", { name: "+ Remote profile" }).first().click();
  await page.getByLabel("Allow invalid TLS certificates for downloads").uncheck();
  await page.getByRole("button", { name: "Download and import" }).click();
  await expect(page.getByRole("heading", { name: "RemoteDraft" })).toBeVisible();

  for (const profile of ["LocalDraft", "RemoteDraft"]) {
    const item = page.locator("article.profile").filter({ has: page.getByRole("heading", { name: profile }) });
    await item.getByRole("button", { name: `Actions ${profile}` }).click();
    await item.getByRole("button", { name: `Delete profile ${profile}` }).click();
    await item.getByRole("button", { name: `Confirm deletion ${profile}` }).click();
  }
  await expect(page.getByText("No profiles yet. Import a YAML file to get started.")).toBeVisible();
});

test("profile metadata editor keeps remote draft across language changes and saves it", async ({ page }) => {
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await page.getByRole("button", { name: "+ 远程订阅" }).first().click();
  await page.getByLabel("订阅链接").fill(`${subscriptionUrl}/ok`);
  await page.getByLabel("远程订阅名称（可选）").fill("MetadataDraft");
  await page.getByRole("button", { name: "下载并导入" }).click();
  const draft = page.locator("article.profile").filter({ has: page.getByRole("heading", { name: "MetadataDraft" }) });
  await expect(draft).toBeVisible();
  const uid = await draft.locator(".mono").textContent();
  const before = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  await draft.getByRole("button", { name: "操作 MetadataDraft" }).click();
  await draft.getByRole("button", { name: "编辑订阅 MetadataDraft" }).click();
  await page.getByLabel("修改订阅名称").fill("MetadataSaved");
  await page.getByLabel("订阅描述").fill("localized editor draft");
  await page.getByLabel("订阅 User-Agent").fill("metadata-i18n-agent");
  await page.getByLabel("下载超时（秒）").fill("7");
  await page.getByLabel("更新间隔（分钟）").fill("60");
  await page.getByLabel("允许自动更新").uncheck();
  await page.getByLabel("订阅刷新允许无效 TLS 证书").check();

  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("heading", { name: "Edit profile" })).toBeVisible();
  await expect(page.getByLabel("Change profile name")).toHaveValue("MetadataSaved");
  await expect(page.getByLabel("Profile description")).toHaveValue("localized editor draft");
  await expect(page.getByLabel("Profile User-Agent")).toHaveValue("metadata-i18n-agent");
  await expect(page.getByLabel("Download timeout (seconds)")).toHaveValue("7");
  await expect(page.getByLabel("Update interval (minutes)")).toHaveValue("60");
  await expect(page.getByLabel("Allow automatic updates")).not.toBeChecked();
  await expect(page.getByLabel("Allow invalid TLS certificates for refreshes")).toBeChecked();
  await page.getByRole("button", { name: "Save profile details" }).click();
  const saved = page.locator("article.profile").filter({ has: page.getByRole("heading", { name: "MetadataSaved" }) });
  await expect(saved).toContainText("localized editor draft");
  const catalog = await fetch(`${base}/api/profiles`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  const item = catalog.items.find((entry: { uid: string }) => entry.uid === uid);
  expect(item.option.user_agent).toBe("metadata-i18n-agent");
  expect(item.option.timeout_seconds).toBe(7);
  expect(item.option.update_interval).toBe(60);
  expect(item.option.allow_auto_update).toBe(false);
  expect(item.option.danger_accept_invalid_certs).toBe(true);
  const after = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  expect(after.generation).toBe(before.generation);
  await saved.getByRole("button", { name: "Actions MetadataSaved" }).click();
  await saved.getByRole("button", { name: "Delete profile MetadataSaved" }).click();
  await saved.getByRole("button", { name: "Confirm deletion MetadataSaved" }).click();
  await expect(page.getByText("No profiles yet. Import a YAML file to get started.")).toBeVisible();
});

test("raw profile editor retranslates feedback and preserves draft across language changes", async ({ page }) => {
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await page.getByRole("button", { name: "+ 本地订阅" }).first().click();
  await page.getByLabel("订阅名称", { exact: true }).fill("RawLanguage");
  await page.getByLabel("订阅 YAML", { exact: true }).fill("proxies: []\nmode: direct\n");
  await page.getByRole("button", { name: "导入订阅", exact: true }).click();
  const profile = page.locator("article.profile").filter({ has: page.getByRole("heading", { name: "RawLanguage" }) });
  await expect(profile).toBeVisible();
  const before = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  const failRead = async (route: import("@playwright/test").Route) => {
    if (route.request().postDataJSON()?.command === "profile_raw")
      await route.fulfill({ status: 503, contentType: "application/json", body: JSON.stringify({ error: { message: "fixture raw read failed" } }) });
    else await route.continue();
  };
  await page.route("**/api/commands", failRead);
  await profile.getByRole("button", { name: "操作 RawLanguage" }).click();
  await profile.getByRole("button", { name: "编辑原始订阅 RawLanguage" }).click();
  await expect(page.getByRole("region", { name: "原始订阅编辑器" }).getByRole("alert")).toContainText("读取原始订阅失败");
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  const english = page.getByRole("region", { name: "Raw profile editor" });
  await expect(english.getByRole("alert")).toContainText("Failed to read the raw profile: fixture raw read failed");
  await page.unroute("**/api/commands", failRead);
  await english.getByRole("button", { name: "Retry reading raw profile" }).click();
  const draft = "# translated draft\nproxies: []\nmode: direct\n";
  await english.getByRole("textbox", { name: "Raw profile YAML" }).fill(draft);
  await english.getByRole("button", { name: "Reload raw profile" }).click();
  await expect(english.getByRole("group", { name: "Confirm raw draft replacement" })).toContainText("Reloading replaces the draft");
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  const chinese = page.getByRole("region", { name: "原始订阅编辑器" });
  await expect(chinese.getByRole("group", { name: "原始订阅草稿替换确认" })).toContainText("重新读取会用服务当前原始内容替换草稿。");
  await expect(chinese.getByRole("textbox", { name: "原始订阅 YAML" })).toHaveValue(draft);
  await chinese.getByRole("button", { name: "继续编辑原始订阅" }).click();
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await english.getByRole("button", { name: "Verify raw profile" }).click();
  await expect(page.locator(".toast").filter({ hasText: "Content differs. Reload it." }).last()).toBeVisible();
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.locator(".toast").filter({ hasText: "内容不同，请重新读取。" }).last()).toBeVisible();
  await expect(chinese.getByRole("textbox", { name: "原始订阅 YAML" })).toHaveValue(draft);
  const after = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  expect(after.generation).toBe(before.generation);
  await chinese.getByRole("button", { name: "关闭原始编辑器" }).click();
  await chinese.getByRole("button", { name: "确认丢弃原始草稿" }).click();
  await profile.getByRole("button", { name: "操作 RawLanguage" }).click();
  await profile.getByRole("button", { name: "删除订阅 RawLanguage" }).click();
  await profile.getByRole("button", { name: "确认删除 RawLanguage" }).click();
  await expect(page.getByText("还没有订阅。导入一个 YAML 文件开始使用。")).toBeVisible();
});

test("profile merge editor keeps its YAML draft when language changes", async ({ page }) => {
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await page.getByRole("button", { name: "+ 本地订阅" }).first().click();
  await page.getByLabel("订阅名称", { exact: true }).fill("MergeLanguage");
  await page.getByLabel("订阅 YAML", { exact: true }).fill("proxies: []\nmode: direct\n");
  await page.getByRole("button", { name: "导入订阅", exact: true }).click();
  const profile = page.locator("article.profile").filter({ has: page.getByRole("heading", { name: "MergeLanguage" }) });
  await expect(profile).toBeVisible();
  const uid = await profile.locator(".mono").textContent();
  const before = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  await profile.getByRole("button", { name: "操作 MergeLanguage" }).click();
  await profile.getByRole("button", { name: "合并增强 MergeLanguage" }).click();
  const merge = "mode: direct\n";
  await page.getByLabel("合并增强 YAML").fill(merge);
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("heading", { name: "Config merge" })).toBeVisible();
  await expect(page.getByText("Merge YAML into MergeLanguage.", { exact: false })).toBeVisible();
  await expect(page.getByLabel("Config merge YAML")).toHaveValue(merge);
  await page.getByRole("button", { name: "Save merge" }).click();
  await expect(profile).toContainText("Config merge linked");
  const response = await fetch(`${base}/api/commands`, {
    method: "POST",
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    body: JSON.stringify({ command: "profile_merge", uid }),
  });
  expect(response.ok).toBe(true);
  expect((await response.json()).yaml).toBe(merge);
  await profile.getByRole("button", { name: "Actions MergeLanguage" }).click();
  await profile.getByRole("button", { name: "Config merge MergeLanguage" }).click();
  await expect(page.getByLabel("Config merge YAML")).toHaveValue(merge);
  await page.getByRole("button", { name: "Remove merge" }).click();
  await expect(profile.getByText("Config merge linked")).toHaveCount(0);
  const after = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  expect(after.generation).toBe(before.generation);
  await profile.getByRole("button", { name: "Actions MergeLanguage" }).click();
  await profile.getByRole("button", { name: "Delete profile MergeLanguage" }).click();
  await profile.getByRole("button", { name: "Confirm deletion MergeLanguage" }).click();
  await expect(page.getByText("No profiles yet. Import a YAML file to get started.")).toBeVisible();
});

test("profile sequence editor localizes kinds and keeps YAML across language changes", async ({ page }) => {
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await page.getByRole("button", { name: "+ 本地订阅" }).first().click();
  await page.getByLabel("订阅名称", { exact: true }).fill("SequenceLanguage");
  await page.getByLabel("订阅 YAML", { exact: true }).fill("proxies: []\nmode: direct\n");
  await page.getByRole("button", { name: "导入订阅", exact: true }).click();
  const profile = page.locator("article.profile").filter({ has: page.getByRole("heading", { name: "SequenceLanguage" }) });
  await expect(profile).toBeVisible();
  const uid = await profile.locator(".mono").textContent();
  const before = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  await profile.getByRole("button", { name: "操作 SequenceLanguage" }).click();
  await profile.getByRole("button", { name: "序列增强 SequenceLanguage" }).click();
  const rules = "prepend: ['DOMAIN,language.test,DIRECT']\nappend: []\ndelete: []\n";
  await page.getByLabel("序列增强 YAML").fill(rules);
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("heading", { name: "Sequence enhancement" })).toBeVisible();
  await expect(page.getByLabel("Sequence YAML")).toHaveValue(rules);
  await expect(page.getByLabel("Sequence type").locator("option")).toHaveText(["Rules", "Proxies", "Proxy groups"]);
  await page.getByRole("button", { name: "Save sequence" }).click();
  await expect(profile).toContainText("Sequence linked");
  const response = await fetch(`${base}/api/commands`, {
    method: "POST",
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    body: JSON.stringify({ command: "profile_sequence", uid, kind: "rules" }),
  });
  expect(response.ok).toBe(true);
  expect((await response.json()).yaml).toBe(rules);
  await profile.getByRole("button", { name: "Actions SequenceLanguage" }).click();
  await profile.getByRole("button", { name: "Sequence enhancement SequenceLanguage" }).click();
  await page.getByLabel("Sequence type").selectOption("groups");
  await expect(page.getByLabel("Sequence type")).toHaveValue("groups");
  await page.getByLabel("Sequence type").selectOption("rules");
  await expect(page.getByLabel("Sequence YAML")).toHaveValue(rules);
  await page.getByRole("button", { name: "Remove sequence" }).click();
  await expect(page.getByLabel("Sequence YAML")).toHaveCount(0);
  await profile.getByRole("button", { name: "Actions SequenceLanguage" }).click();
  await profile.getByRole("button", { name: "Sequence enhancement SequenceLanguage" }).click();
  await expect(page.getByRole("button", { name: "Remove sequence" })).toBeDisabled();
  await page.getByRole("button", { name: "Cancel sequence editing" }).click();
  const after = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  expect(after.generation).toBe(before.generation);
  await profile.getByRole("button", { name: "Actions SequenceLanguage" }).click();
  await profile.getByRole("button", { name: "Delete profile SequenceLanguage" }).click();
  await profile.getByRole("button", { name: "Confirm deletion SequenceLanguage" }).click();
  await expect(page.getByText("No profiles yet. Import a YAML file to get started.")).toBeVisible();
});

test("profile script editor preserves JavaScript draft across language changes", async ({ page }) => {
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await page.getByRole("button", { name: "+ 本地订阅" }).first().click();
  await page.getByLabel("订阅名称", { exact: true }).fill("ScriptLanguage");
  await page.getByLabel("订阅 YAML", { exact: true }).fill("proxies: []\nmode: direct\n");
  await page.getByRole("button", { name: "导入订阅", exact: true }).click();
  const profile = page.locator("article.profile").filter({ has: page.getByRole("heading", { name: "ScriptLanguage" }) });
  await expect(profile).toBeVisible();
  const uid = await profile.locator(".mono").textContent();
  const before = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  await profile.getByRole("button", { name: "操作 ScriptLanguage" }).click();
  await profile.getByRole("button", { name: "脚本增强 ScriptLanguage" }).click();
  const source = "function main(config, name) {\n  // localized draft\n  return config;\n}\n";
  await page.getByLabel("脚本增强 JavaScript").fill(source);
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("heading", { name: "Script enhancement" })).toBeVisible();
  await expect(page.getByText("Write main(config, name) for ScriptLanguage", { exact: false })).toBeVisible();
  await expect(page.getByLabel("Script enhancement JavaScript")).toHaveValue(source);
  await page.getByRole("button", { name: "Save script enhancement" }).click();
  await expect(page.getByRole("heading", { name: "Script enhancement", exact: true })).toHaveCount(0);
  await expect(profile).toContainText("Script linked");
  const response = await fetch(`${base}/api/commands`, {
    method: "POST",
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    body: JSON.stringify({ command: "profile_script", uid }),
  });
  expect(response.ok).toBe(true);
  expect((await response.json()).source).toBe(source);
  await profile.getByRole("button", { name: "Actions ScriptLanguage" }).click();
  await profile.getByRole("button", { name: "Script enhancement ScriptLanguage" }).click();
  await expect(page.getByLabel("Script enhancement JavaScript")).toHaveValue(source);
  await page.getByRole("button", { name: "Remove script enhancement" }).click();
  await expect(profile.getByText("Script linked")).toHaveCount(0);
  const after = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  expect(after.generation).toBe(before.generation);
  await profile.getByRole("button", { name: "Actions ScriptLanguage" }).click();
  await profile.getByRole("button", { name: "Delete profile ScriptLanguage" }).click();
  await profile.getByRole("button", { name: "Confirm deletion ScriptLanguage" }).click();
  await expect(page.getByText("No profiles yet. Import a YAML file to get started.")).toBeVisible();
});

test("global merge editor translates without replacing its draft or reset confirmation", async ({ page }) => {
  const headers = { Authorization: `Bearer ${token}` };
  const readMerge = async () => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: { ...headers, "Content-Type": "application/json" },
      body: JSON.stringify({ command: "global_merge" }),
    });
    expect(response.ok).toBe(true);
    return (await response.json()).yaml as string;
  };
  const original = await readMerge();
  const before = await fetch(`${base}/api/status`, { headers }).then(response => response.json());
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await page.getByRole("button", { name: "编辑全局合并" }).click();
  const merge = "# bilingual global merge\nmode: direct\n";
  await page.getByLabel("全局合并 YAML").fill(merge);
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("region", { name: "Global enhancements" })).toBeVisible();
  await expect(page.getByRole("form", { name: "Global merge enhancement" })).toBeVisible();
  await expect(page.getByLabel("Global merge YAML")).toHaveValue(merge);
  await page.getByRole("button", { name: "Restore default global merge" }).click();
  await expect(page.getByRole("group", { name: "Confirm restoring default" })).toContainText("replacing the saved merge and current input");
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.getByRole("group", { name: "恢复默认确认" })).toContainText("替换已保存的合并和当前输入");
  await expect(page.getByLabel("全局合并 YAML")).toHaveValue(merge);
  await page.getByRole("button", { name: "继续编辑" }).click();
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await page.getByRole("button", { name: "Save global merge" }).click();
  await expect(page.getByLabel("Global merge YAML")).toHaveCount(0);
  expect(await readMerge()).toBe(merge);
  await page.getByRole("button", { name: "Edit global merge" }).click();
  await expect(page.getByLabel("Global merge YAML")).toHaveValue(merge);
  await page.getByRole("button", { name: "Restore default global merge" }).click();
  await page.getByRole("button", { name: "Confirm restore default" }).click();
  expect(await readMerge()).toBe(original);
  const after = await fetch(`${base}/api/status`, { headers }).then(response => response.json());
  expect(after.generation).toBe(before.generation);
});

test("global script editor translates draft, size feedback and reset confirmation", async ({ page }) => {
  const headers = { Authorization: `Bearer ${token}` };
  const readScript = async () => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: { ...headers, "Content-Type": "application/json" },
      body: JSON.stringify({ command: "global_script" }),
    });
    expect(response.ok).toBe(true);
    return (await response.json()).source as string;
  };
  const original = await readScript();
  const before = await fetch(`${base}/api/status`, { headers }).then(response => response.json());
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await page.getByRole("button", { name: "编辑全局脚本" }).click();
  const source = "// bilingual global script\nfunction main(config, name) { return config; }\n";
  await page.getByLabel("全局脚本 JavaScript").fill(source);
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("form", { name: "Global script enhancement" })).toBeVisible();
  await expect(page.getByLabel("Global script JavaScript")).toHaveValue(source);
  await page.getByRole("button", { name: "Restore default global script" }).click();
  await expect(page.getByRole("group", { name: "Confirm restoring default" })).toContainText("replacing the saved script and current input");
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.getByRole("group", { name: "恢复默认确认" })).toContainText("替换已保存的脚本和当前输入");
  await expect(page.getByLabel("全局脚本 JavaScript")).toHaveValue(source);
  await page.getByRole("button", { name: "继续编辑" }).click();
  await page.getByLabel("全局脚本 JavaScript").fill("x".repeat(1024 * 1024 + 1));
  await page.getByRole("button", { name: "保存全局脚本" }).click();
  await expect(page.getByRole("alert")).toContainText("脚本不能超过 1 MiB");
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("alert")).toContainText("Script must not exceed 1 MiB");
  await page.getByLabel("Global script JavaScript").fill(source);
  await page.getByRole("button", { name: "Save global script" }).click();
  await expect(page.getByLabel("Global script JavaScript")).toHaveCount(0);
  expect(await readScript()).toBe(source);
  await page.getByRole("button", { name: "Edit global script" }).click();
  await expect(page.getByLabel("Global script JavaScript")).toHaveValue(source);
  await page.getByRole("button", { name: "Restore default global script" }).click();
  await page.getByRole("button", { name: "Confirm restore default" }).click();
  expect(await readScript()).toBe(original);
  const after = await fetch(`${base}/api/status`, { headers }).then(response => response.json());
  expect(after.generation).toBe(before.generation);
});

test("resource inventory reads metadata, refreshes changes and retries without editing settings", async ({ page }) => {
  await page.goto(`${base}/settings`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await openSettingsForEditing(page);
  await page.locator(".settings-details").filter({ has: page.locator("summary", { hasText: /内核实际设置|资源与数据库/ }) }).evaluateAll(nodes => nodes.forEach(node => (node as HTMLDetailsElement).open = true));
  const panel = page.getByRole("region", { name: "运行资源清单", exact: true });
  await expect(panel).toContainText("Geo 文件");
  await expect(panel).toContainText("当前已提交配置没有 Provider 声明。");
  await expect(panel).toContainText("尚无已提交配置");
  const country = panel.locator("li").filter({ has: page.getByText("Country.mmdb", { exact: true }) });
  await expect(country).toContainText("文件缺失");
  await writeFile(join(directory, "Country.mmdb"), "metadata fixture");
  try {
    await panel.getByRole("button", { name: "刷新资源清单" }).click();
    await expect(country).toContainText("文件存在");
    await expect(country).toContainText("16 字节");
    await country.getByRole("button", { name: "校验 Country.mmdb", exact: true }).click();
    await expect(country.getByRole("alert")).toContainText("invalid MMDB");
    expect(await readFile(join(directory, "Country.mmdb"), "utf8")).toBe("metadata fixture");
    await page.route("**/api/commands", async route => {
      if (route.request().postDataJSON().command === "validate_geo") {
        await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ verified: true, warning: null, sha256: "a".repeat(64), bytes: 16, ip_version: 4, node_count: 1 }) });
      } else await route.continue();
    });
    await country.getByRole("button", { name: "校验 Country.mmdb", exact: true }).click();
    await expect(country.getByRole("status")).toContainText("MMDB 结构校验通过");
    await page.unroute("**/api/commands");
    await page.route("**/api/commands", async route => {
      if (route.request().postDataJSON().command === "validate_geo") {
        await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ verified: false, warning: "empty_description_structure_unverified", sha256: "a".repeat(64), bytes: 16, ip_version: 4, node_count: 1 }) });
      } else await route.continue();
    });
    await country.getByRole("button", { name: "校验 Country.mmdb", exact: true }).click();
    await expect(country.getByRole("status")).toContainText("完整结构未验证");
    await expect(country).not.toContainText("MMDB 结构校验通过");
    await page.unroute("**/api/commands");
    let failed = false;
    await page.route("**/api/commands", async route => {
      if (route.request().postDataJSON().command === "resources" && !failed) {
        failed = true;
        await route.fulfill({ status: 422, contentType: "application/json", body: JSON.stringify({ error: { message: "fixture resource failure" } }) });
      } else await route.continue();
    });
    await panel.getByRole("button", { name: "刷新资源清单" }).click();
    await expect(panel.getByRole("alert")).toContainText("fixture resource failure");
    await writeFile(join(directory, "Country.mmdb"), "");
    await panel.getByRole("button", { name: "刷新资源清单" }).click();
    await expect(country).toContainText("空文件");
    await expect(country.getByRole("status")).toHaveCount(0);
    await expect(country.getByRole("button", { name: "校验 Country.mmdb", exact: true })).toHaveCount(0);
    await expect(country.getByRole("button", { name: "读取 Country.mmdb 在线来源", exact: true })).toBeVisible();
    await expect(panel.getByRole("alert")).toHaveCount(0);
    await panel.getByRole("button", { name: "Geo / Provider 资源", exact: true }).hover();
    await expect(page.getByRole("tooltip")).toContainText("尚未验证内容格式");
    await page.unroute("**/api/commands");
    await page.getByRole("button", { name: "退出登录" }).click();
  } finally { await rm(join(directory, "Country.mmdb"), { force: true }); }
});

test("DAT resource checks show structural counts, unknown fields and stale result clearing", async ({ page }) => {
  const vint = (n: number): number[] => { const out: number[] = []; while (n > 127) { out.push((n & 127) | 128); n >>>= 7; } return [...out, n]; };
  const field = (n: number, bytes: number[]) => [...vint((n << 3) | 2), ...vint(bytes.length), ...bytes];
  const string = (s: string) => [...Buffer.from(s)];
  const group = (records: number[][]) => Buffer.from(field(1, [...field(1, string("browser")), ...records.flatMap(r => field(2, r))]));
  const geoip = group([[...field(1, [192, 0, 2, 0]), 16, 24], [...field(1, [32, 1, 13, 184, ...Array(12).fill(0)]), 16, 32]]);
  const geosite = group([[8, 3, ...field(2, string("exact.dat.test")), ...field(3, [...field(1, string("test")), 16, 1])], [8, 1, ...field(2, string("^.*$"))]]);
  await page.goto(`${base}/settings`); await page.getByLabel("管理令牌").fill(token); await page.getByRole("button", { name: "连接服务" }).click();
  await openSettingsForEditing(page);
  await page.locator(".settings-details").filter({ has: page.locator("summary", { hasText: /内核实际设置|资源与数据库/ }) }).evaluateAll(nodes => nodes.forEach(node => (node as HTMLDetailsElement).open = true));
  const panel = page.getByRole("region", { name: "运行资源清单", exact: true });
  const ip = panel.locator("li").filter({ has: page.getByText("geoip.dat", { exact: true }) });
  const site = panel.locator("li").filter({ has: page.getByText("geosite.dat", { exact: true }) });
  try {
    await writeFile(join(directory, "geoip.dat"), geoip); await writeFile(join(directory, "geosite.dat"), geosite);
    await panel.getByRole("button", { name: "刷新资源清单" }).click();
    await ip.getByRole("button", { name: "校验 geoip.dat", exact: true }).click();
    await expect(ip.getByRole("status")).toContainText("DAT 已知结构校验通过");
    await expect(ip.getByRole("status")).toContainText("1 分组 · 2 记录 · IPv4 1 / IPv6 1");
    await expect(ip.getByRole("status")).toContainText(createHash("sha256").update(geoip).digest("hex"));
    await expect(ip.getByRole("status")).toContainText("兼容性未验证");
    await expect(ip.getByRole("status")).toContainText("缺少 CN 分组");
    expect(await readFile(join(directory, "geoip.dat"))).toEqual(geoip);
    await site.getByRole("button", { name: "校验 geosite.dat", exact: true }).click();
    await expect(site.getByRole("status")).toContainText("正则 1 · 属性 1");
    const future = Buffer.concat([geosite, Buffer.from([32, 1])]); await writeFile(join(directory, "geosite.dat"), future);
    await site.getByRole("button", { name: "校验 geosite.dat", exact: true }).click();
    await expect(site.getByRole("status")).toContainText("未知字段，完整结构未验证");
    await expect(site.getByRole("status")).toContainText("未知字段 1");
    await expect(site).not.toContainText("已知结构校验通过");
    await writeFile(join(directory, "geosite.dat"), Buffer.from([10, 255]));
    await site.getByRole("button", { name: "校验 geosite.dat", exact: true }).click();
    await expect(site.getByRole("alert")).toContainText("校验失败"); await expect(site.getByRole("status")).toHaveCount(0);
    await writeFile(join(directory, "geosite.dat"), geosite);
    await site.getByRole("button", { name: "校验 geosite.dat", exact: true }).click(); await expect(site.getByRole("status")).toContainText("已知结构校验通过");
    await panel.getByRole("button", { name: "刷新资源清单" }).click(); await expect(site.getByRole("status")).toHaveCount(0); await expect(ip.getByRole("status")).toHaveCount(0);
    await page.getByRole("button", { name: "退出登录", exact: true }).click();
  } finally { await rm(join(directory, "geoip.dat"), { force: true }); await rm(join(directory, "geosite.dat"), { force: true }); }
});

test("Geo bundle update requests guard hashes, require fresh inspection and retain warnings", async ({ page }) => {
  let reads = 0, installs = 0;
  let sendPhase: (phase: string) => void = () => { throw new Error("WebSocket fixture not ready"); };
  await page.routeWebSocket("**/api/events", socket => {
    sendPhase = phase => socket.send(JSON.stringify({ type: "status", data: { phase, generation: 0, selection_pending: [] } }));
    socket.onMessage(message => {
      if (JSON.parse(String(message)).type === "authenticate") {
        socket.send(JSON.stringify({ type: "ready" }));
        sendPhase("running");
      }
    });
  });
  const seedHash = "a".repeat(64), oldHash = "b".repeat(64), newHash = "c".repeat(64);
  await page.route("**/api/commands", async route => {
    const body = route.request().postDataJSON();
    if (body.command === "resources") {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ data_dir: directory, bundle_dir: "/fixture/bundle", config_revision: null, geo: [{ section: "geo", name: "Country.mmdb", state: "available", path: "Country.mmdb", provider_type: null, bytes: installs > 1 ? 32 : 16, conflict: false }], providers: [] }) });
    } else if (body.command === "geo_seed") {
      reads++;
      expect(body.name).toBe("Country.mmdb");
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ name: body.name, current_sha256: reads < 3 ? oldHash : newHash, seed_sha256: seedHash, seed_bytes: 32 }) });
    } else if (body.command === "install_geo_seed") {
      installs++;
      expect(body).toEqual({ command: "install_geo_seed", name: "Country.mmdb", expected_current_sha256: installs === 1 ? oldHash : newHash, expected_seed_sha256: seedHash, accept_metadata_only: installs > 1 });
      await route.fulfill({ status: installs === 1 ? 422 : 200, contentType: "application/json", body: JSON.stringify(installs === 1 ? { error: { message: "Geo file changed since update inspection" } } : { changed: true, durable: true, cleanup_pending: false, validation: { verified: false, sha256: seedHash } }) });
    } else await route.continue();
  });
  await page.goto(`${base}/settings`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await openSettingsForEditing(page);
  await page.locator(".settings-details").filter({ has: page.locator("summary", { hasText: /内核实际设置|资源与数据库/ }) }).evaluateAll(nodes => nodes.forEach(node => (node as HTMLDetailsElement).open = true));
  const panel = page.getByRole("region", { name: "运行资源清单", exact: true });
  const read = panel.getByRole("button", { name: "读取 Country.mmdb 打包更新", exact: true });
  await read.click();
  const install = panel.getByRole("button", { name: "安装 Country.mmdb 打包资源", exact: true });
  await expect(install).toBeDisabled();
  await expect(panel).toContainText("停止内核后可安装打包资源");
  sendPhase("stopped");
  await expect(install).toHaveCount(0);
  await read.click();
  await expect(install).toBeEnabled();
  await install.click();
  await expect(panel.getByRole("alert")).toContainText("Geo file changed");
  await expect(install).toHaveCount(0);
  await read.click();
  await panel.getByRole("checkbox", { name: "允许安装描述为空、完整结构未验证的 MMDB" }).check();
  await install.click();
  await expect(page.locator(".toast").filter({ hasText: "已安装打包资源" }).last()).toBeVisible();
  await expect(page.locator(".toast").filter({ hasText: "完整结构未验证" }).last()).toBeVisible();
  await expect(panel).toContainText("32 字节");
  await expect(install).toHaveCount(0);
  expect(reads).toBe(3); expect(installs).toBe(2);
  await page.unroute("**/api/commands");
  await page.getByRole("button", { name: "退出登录" }).click();
});

test("pinned DAT install requires a fresh stopped-core inspection and shows core probe receipt", async ({ page }) => {
  let phase: (value: string) => void = () => { throw new Error("socket not ready"); };
  let reads = 0, installs = 0;
  const seedHash = "a".repeat(64), oldHash = "b".repeat(64);
  await page.routeWebSocket("**/api/events", socket => {
    phase = value => socket.send(JSON.stringify({ type: "status", data: { phase: value, generation: 0, selection_pending: [] } }));
    socket.onMessage(message => {
      if (JSON.parse(String(message)).type === "authenticate") {
        socket.send(JSON.stringify({ type: "ready" })); phase("running");
      }
    });
  });
  await page.route("**/api/commands", async route => {
    const body = route.request().postDataJSON();
    if (body.command === "resources") {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ data_dir: directory, bundle_dir: "/fixture/bundle", config_revision: null, geo: [{ section: "geo", name: "geosite.dat", state: "available", path: "geosite.dat", provider_type: null, bytes: 42, conflict: false }], providers: [] }) });
    } else if (body.command === "geo_seed") {
      reads++; expect(body.name).toBe("geosite.dat");
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ name: body.name, current_sha256: oldHash, seed_sha256: seedHash, seed_bytes: 42 }) });
    } else if (body.command === "install_geo_seed") {
      installs++; expect(body).toEqual({ command: "install_geo_seed", name: "geosite.dat", expected_current_sha256: oldHash, expected_seed_sha256: seedHash, accept_metadata_only: false });
      await route.fulfill({ status: installs === 1 ? 422 : 200, contentType: "application/json", body: JSON.stringify(installs === 1 ? { error: { message: "isolated Mihomo DAT compatibility probe failed" } } : { changed: true, durable: true, cleanup_pending: false, core_load_verified: true, validation: { verified: true, sha256: seedHash, format: "dat" } }) });
    } else await route.continue();
  });
  await page.goto(`${base}/settings`); await page.getByLabel("管理令牌").fill(token); await page.getByRole("button", { name: "连接服务" }).click();
  await openSettingsForEditing(page);
  await page.locator(".settings-details").filter({ has: page.locator("summary", { hasText: /内核实际设置|资源与数据库/ }) }).evaluateAll(nodes => nodes.forEach(node => (node as HTMLDetailsElement).open = true));
  const panel = page.getByRole("region", { name: "运行资源清单", exact: true });
  const read = panel.getByRole("button", { name: "读取 geosite.dat 打包更新", exact: true });
  await read.click();
  const install = panel.getByRole("button", { name: "安装 geosite.dat 打包资源", exact: true });
  await expect(install).toBeDisabled();
  await expect(panel.getByRole("checkbox", { name: "允许安装描述为空、完整结构未验证的 MMDB" })).toHaveCount(0);
  phase("stopped"); await expect(install).toHaveCount(0);
  await read.click(); await expect(install).toBeEnabled();
  await install.click(); await expect(panel.getByRole("alert")).toContainText("compatibility probe failed");
  await expect(install).toHaveCount(0);
  await read.click(); await install.click();
  await expect(page.locator(".toast").filter({ hasText: "DAT 结构及隔离内核规则加载通过" }).last()).toBeVisible();
  await expect(page.locator(".toast").filter({ hasText: seedHash }).last()).toBeVisible();
  expect(reads).toBe(3); expect(installs).toBe(2);
  await page.unroute("**/api/commands"); await page.getByRole("button", { name: "退出登录" }).click();
});

test("online Geo update preserves inspection across running-core restart and retries failures", async ({ page }) => {
  let phase: (value: string) => void = () => { throw new Error("socket not ready"); };
  let reads = 0, updates = 0;
  const sourceHash = "a".repeat(64), oldHash = "b".repeat(64), newHash = "c".repeat(64);
  await page.routeWebSocket("**/api/events", socket => {
    phase = value => socket.send(JSON.stringify({ type: "status", data: { phase: value, generation: 0, selection_pending: [] } }));
    socket.onMessage(message => {
      if (JSON.parse(String(message)).type === "authenticate") {
        socket.send(JSON.stringify({ type: "ready" })); phase("running");
      }
    });
  });
  await page.route("**/api/commands", async route => {
    const body = route.request().postDataJSON();
    if (body.command === "resources") {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ data_dir: directory, bundle_dir: null, config_revision: "one", geo: [{ section: "geo", name: "geosite.dat", state: "available", path: "geosite.dat", provider_type: null, bytes: 42, conflict: false }], providers: [] }) });
    } else if (body.command === "geo_online_info") {
      reads++; expect(body.name).toBe("geosite.dat");
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ name: body.name, current_sha256: oldHash, source_sha256: sourceHash }) });
    } else if (body.command === "update_geo_online") {
      updates++; expect(body).toEqual({ command: "update_geo_online", name: "geosite.dat", expected_current_sha256: oldHash, expected_source_sha256: sourceHash, expected_download_sha256: newHash, accept_metadata_only: false, route: "managed", danger_accept_invalid_certs: true });
      if (updates === 2) { phase("stopping"); phase("starting"); phase("running"); }
      await route.fulfill({ status: updates === 1 ? 422 : 200, contentType: "application/json", body: JSON.stringify(updates === 1 ? { error: { message: "Geo download SHA-256 differs from expected pin" } } : { changed: true, durable: true, cleanup_pending: false, core_load_verified: true, validation: { verified: true, sha256: newHash, format: "dat" } }) });
    } else await route.continue();
  });
  await page.goto(`${base}/settings`); await page.getByLabel("管理令牌").fill(token); await page.getByRole("button", { name: "连接服务" }).click();
  await openSettingsForEditing(page);
  await page.locator(".settings-details").filter({ has: page.locator("summary", { hasText: /内核实际设置|资源与数据库/ }) }).evaluateAll(nodes => nodes.forEach(node => (node as HTMLDetailsElement).open = true));
  const panel = page.getByRole("region", { name: "运行资源清单", exact: true });
  const read = panel.getByRole("button", { name: "读取 geosite.dat 在线来源", exact: true });
  await read.click();
  const update = panel.getByRole("button", { name: "更新 geosite.dat 在线资源", exact: true });
  await expect(update).toBeEnabled();
  await expect(panel).toContainText("短暂停止核心");
  await expect(panel).toContainText(sourceHash);
  await panel.getByRole("combobox", { name: "下载路由" }).selectOption("managed");
  await panel.getByRole("checkbox", { name: "显式忽略下载来源证书错误" }).check();
  await panel.getByRole("textbox", { name: "可选下载 SHA-256" }).fill("bad");
  await update.click(); await expect(panel.getByRole("alert")).toContainText("64 位十六进制");
  expect(updates).toBe(0);
  await panel.getByRole("textbox", { name: "可选下载 SHA-256" }).fill(newHash);
  await update.click(); await expect(panel.getByRole("alert")).toContainText("differs from expected pin");
  await expect(update).toHaveCount(0);
  await read.click(); await panel.getByRole("textbox", { name: "可选下载 SHA-256" }).fill(newHash);
  await update.click(); await expect(page.locator(".toast").filter({ hasText: "已安装在线资源" }).last()).toBeVisible();
  await expect(page.locator(".toast").filter({ hasText: newHash }).last()).toBeVisible();
  expect(reads).toBe(2); expect(updates).toBe(2);
  await page.unroute("**/api/commands"); await page.getByRole("button", { name: "退出登录" }).click();
});

test("connection settings save explicit values and preserve other settings", async ({ page }) => {
  const original = await settingsApi("settings");
  try {
    await loginSettings(page);
    for (const [name, value, key] of [
      ["TCP 并发连接", "false", "tcp-concurrent"], ["进程匹配模式", "off", "find-process-mode"],
      ["禁用 TCP 保活", "false", "disable-keep-alive"], ["IPv6", "false", "ipv6"],
      ["核心下载 ETag", "false", "etag-support"],
    ]) {
      await editSetting(page, name, value, true);
      expect((await settingsApi("settings")).runtime[key]).toBe(value === "false" ? false : value);
    }
    for (const [name, value, key] of [
      ["TCP 保活间隔（秒）", "0", "keep-alive-interval"], ["TCP 保活空闲时间（秒）", "-1", "keep-alive-idle"],
      ["Linux 路由标记", "4294967295", "routing-mark"],
    ]) {
      await editSetting(page, name, value);
      expect((await settingsApi("settings")).runtime[key]).toBe(Number(value));
    }
    const before = await settingsApi("settings");
    for (const invalid of ["2147483648", "-2147483649", "1.5", "abc"]) {
      await editSetting(page, "TCP 保活间隔（秒）", invalid);
      await expect(page.getByRole("region", { name: "服务设置编辑器" }).getByRole("alert")).toContainText("必须是 -2147483648–2147483647");
      expect(await settingsApi("settings")).toEqual(before);
    }
    await editSetting(page, "TCP 保活间隔（秒）", "0");
    for (const [managed, name, key, value] of [
      ["管理出口网卡", "出口网卡名称", "interface-name", "lo"],
      ["管理核心下载 User-Agent", "核心下载 User-Agent", "global-ua", "browser-agent/1"],
    ]) {
      const checkbox = page.getByRole("checkbox", { name: managed, exact: true });
      await checkbox.check(); await applySettings(page);
      await editSetting(page, name, value);
      expect((await settingsApi("settings")).runtime[key]).toBe(value);
      await checkbox.uncheck(); await applySettings(page);
      expect((await settingsApi("settings")).runtime[key]).toBeUndefined();
    }
  } finally { await settingsApi("set_settings", { runtime: original.runtime }); }
});

test("hosts editor saves maps, explicit empty values and multiline input", async ({ page }) => {
  const original = await settingsApi("settings");
  const custom = { "*.example.test": "192.0.2.1", "multi.example.test": ["192.0.2.2", "2001:db8::1"], "alias.example.test": "multi.example.test" };
  try {
    await loginSettings(page);
    const owned = page.getByRole("checkbox", { name: "管理 hosts 映射", exact: true });
    await owned.check(); await applySettings(page);
    const before = await settingsApi("settings");
    const hosts = page.getByRole("textbox", { name: "hosts JSON 映射", exact: true });
    for (const invalid of ['[]', '{"a.test":123}', '{"a.test":[]}', '{"a.test":["alias.test"]}', '{"*.test":"a.test"}', '{"a.test":"192.0.2.1","A.TEST":"192.0.2.2"}']) {
      await editSetting(page, "hosts JSON 映射", invalid);
      await expect(page.getByRole("region", { name: "服务设置编辑器" }).getByRole("alert")).toContainText("hosts");
      expect(await settingsApi("settings")).toEqual(before);
    }
    await hosts.fill(JSON.stringify(custom, null, 2));
    await hosts.press("Shift+Enter");
    expect(await settingsApi("settings")).toEqual(before);
    await hosts.press("Enter");
    expect(await settingsApi("settings")).toEqual(before);
    await applySettings(page);
    expect((await settingsApi("settings")).runtime.hosts).toEqual(custom);
    await editSetting(page, "DNS 设置来源", "true", true);
    await editSetting(page, "DNS 使用 hosts", "false", true);
    await editSetting(page, "DNS 使用系统 hosts", "false", true);
    expect((await settingsApi("settings")).runtime.dns).toEqual({ "use-hosts": false, "use-system-hosts": false });
    await page.route("**/api/commands", async route => {
      if (route.request().postDataJSON().command === "set_settings") await route.fulfill({ status: 503, json: { error: { message: "fixture hosts failure" } } });
      else await route.continue();
    });
    await editSetting(page, "hosts JSON 映射", '{}');
    await expect(hosts).toHaveValue('{}');
    expect((await settingsApi("settings")).runtime.hosts).toEqual(custom);
    await page.unroute("**/api/commands");
    await applySettings(page);
    expect((await settingsApi("settings")).runtime.hosts).toEqual({});
    await owned.uncheck(); await applySettings(page);
    expect((await settingsApi("settings")).runtime.hosts).toBeUndefined();
  } finally { await page.unroute("**/api/commands"); await settingsApi("set_settings", { runtime: original.runtime }); }
});

test("Geo settings save inherited URL leaves and validate before submission", async ({ page }) => {
  const original = await settingsApi("settings");
  try {
    await loginSettings(page);
    for (const [name, value] of [["Geo 数据模式", "false"], ["Geo 加载器", "standard"], ["GeoSite 匹配器", "mph"], ["Geo 自动更新", "false"]]) await editSetting(page, name, value, true);
    const before = await settingsApi("settings");
    await editSetting(page, "Geo 更新间隔（小时）", "0");
    await expect(page.getByRole("region", { name: "服务设置编辑器" }).getByRole("alert")).toContainText("1–8760");
    expect(await settingsApi("settings")).toEqual(before);
    await editSetting(page, "Geo 更新间隔（小时）", "48");
    const owned = page.getByRole("checkbox", { name: "管理 Geo 下载地址", exact: true });
    await owned.check(); await applySettings(page);
    const inherited = await settingsApi("settings");
    await editSetting(page, "MMDB 下载地址", "file:///etc/passwd");
    await expect(page.getByRole("region", { name: "服务设置编辑器" }).getByRole("alert")).toContainText("HTTP(S)");
    expect(await settingsApi("settings")).toEqual(inherited);
    await editSetting(page, "MMDB 下载地址", "http://127.0.0.1:1/browser-mmdb");
    const runtime = (await settingsApi("settings")).runtime;
    expect(runtime["geox-url"]).toEqual({ mmdb: "http://127.0.0.1:1/browser-mmdb" });
    expect(runtime["geo-update-interval"]).toBe(48);
    await editSetting(page, "MMDB 下载地址", "");
    expect((await settingsApi("settings")).runtime["geox-url"]).toEqual({});
    await owned.uncheck(); await applySettings(page);
    expect((await settingsApi("settings")).runtime["geox-url"]).toBeUndefined();
  } finally { await settingsApi("set_settings", { runtime: original.runtime }); }
});

test("browser repairs failed startup, saves selection/config, restores after service restart and logs out", async ({
  page,
}) => {
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (
      message.type() === "error" &&
      message.text().includes("Content Security Policy")
    )
      errors.push(message.text());
  });
  await page.goto(base);
  await page.getByLabel("管理令牌").fill("incorrect");
  await page.getByRole("button", { name: "连接服务" }).click();
  await expect(page.getByRole("alert")).toContainText("令牌无效");
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await expect(
    page.getByRole("heading", { name: "概览", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible();
  await expect(page.locator(".sidebar-status")).toContainText("启动失败");
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /设置/ })
    .click();
  await expect(
    page.getByText("尚无已提交配置，保存仅记录设置，首次启动时使用。", {
      exact: true,
    }),
  ).toBeVisible();
  await page
    .getByRole("combobox", { name: "代理模式", exact: true })
    .selectOption("global");
  await applySettings(page);
  await expect(
    page.getByText("保存成功", { exact: true }).last(),
  ).toBeVisible();
  await expect(page.locator(".sidebar-status")).toContainText("启动失败");
  await page.getByRole("combobox", { name: "代理模式", exact: true }).selectOption("");
  await applySettings(page);
  await expect(
    page.getByText("保存成功", { exact: true }).last(),
  ).toBeVisible();
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /订阅/ })
    .click();
  const yaml =
    "mixed-port: 0\nmode: rule\nlog-level: info\nexternal-controller: ''\ndns: {enable: false}\ntun: {enable: false}\nprofile: {store-selected: false}\nproxy-groups:\n  - {name: Main, type: select, proxies: [DIRECT, REJECT]}\nrules: ['MATCH,Main']\n";
  await page.getByRole("button", { name: "+ 本地订阅", exact: true }).click();
  await page.getByLabel("上传订阅 YAML").setInputFiles({
    name: "Browser.yaml",
    mimeType: "text/yaml",
    buffer: Buffer.from(yaml),
  });
  await expect(page.getByLabel("订阅名称", { exact: true })).toHaveValue(
    "Browser",
  );
  await page.getByRole("button", { name: "导入订阅", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Browser", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "使用订阅" }).click();
  await expect(page.getByText("当前订阅", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "+ 远程订阅", exact: true }).click();
  await page
    .getByLabel("订阅链接", { exact: true })
    .fill(`${subscriptionUrl}/ok`);
  await page.getByRole("button", { name: "下载并导入", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "BrowserRemote.yaml", exact: true }),
  ).toBeVisible();
  await expect(page.locator("article.profile")).toHaveCount(2);
  await expect(
    page.locator("article.profile").filter({ hasText: "BrowserRemote.yaml" }),
  ).toContainText("远程订阅");
  await expect(
    page.locator("article.profile").filter({ hasText: "BrowserRemote.yaml" }),
  ).toContainText("已用");
  await page.getByRole("button", { name: "+ 远程订阅", exact: true }).click();
  await page
    .getByLabel("订阅链接", { exact: true })
    .fill(`${subscriptionUrl}/error`);
  await page.getByRole("button", { name: "下载并导入", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("503");
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await expect(page.locator("article.profile")).toHaveCount(2);
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /内核/ }).click();
  await page.getByRole("button", { name: "启动内核" }).click();
  await expect(page.locator(".sidebar-status")).toContainText("运行中");
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /代理/ })
    .click();
  await page
    .getByRole("button", { name: "选择 Main / REJECT", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "选择 Main / REJECT", exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /配置/ })
    .click();
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: rule/);
  await page
    .getByLabel("运行配置 YAML")
    .fill("mode: rule\nrules: ['INVALID,DIRECT']\n");
  await page.getByRole("button", { name: "校验并应用" }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page.locator(".sidebar-status")).toContainText("运行中");
  await page
    .getByLabel("运行配置 YAML")
    .fill(yaml.replace("mode: rule", "mode: direct"));
  await page.getByRole("button", { name: "校验并应用" }).click();
  await expect(page.getByText("保存成功")).toBeVisible();
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /内核/ }).click();
  await page.getByRole("button", { name: "停止内核" }).click();
  await expect(page.locator(".sidebar-status")).toContainText("已停止");
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /配置/ }).click();
  await expect(page.getByLabel("运行配置 YAML")).toBeEnabled();
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /内核/ }).click();
  await page.getByRole("button", { name: "重启内核" }).click();
  await expect(page.locator(".sidebar-status")).toContainText("运行中");
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /日志/ })
    .click();
  await expect(page.getByRole("log")).toContainText("time=");
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /概览/ })
    .click();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible();
  await expect(
    page.locator(".metrics > div").nth(2).locator("strong"),
  ).toHaveText(/^\d+$/);
  await expect(
    page.locator(".metrics > div").nth(3).locator("strong"),
  ).not.toHaveText("—");
  await page.screenshot({ path: "test-results/overview.png", fullPage: true });
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /代理/ })
    .click();
  await expect(
    page.getByRole("button", { name: "选择 Main / REJECT", exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /配置/ })
    .click();
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: direct/);
  await page.reload();
  // The session token is cached per tab, so a refresh keeps the login.
  await expect(page.getByLabel("运行配置 YAML")).toBeVisible();
  expect(
    await page.evaluate(() => Object.keys(localStorage).join()),
  ).not.toContain("token");
  await page.setViewportSize({ width: 390, height: 844 });
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /订阅/ })
    .click();
  await expect(
    page.getByRole("heading", { name: "Browser", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "BrowserRemote.yaml", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({ path: "test-results/mobile.png", fullPage: true });
  await page.getByRole("button", { name: "退出登录" }).click();
  await expect(page.getByLabel("管理令牌")).toHaveValue("");
  await page.reload();
  await expect(
    page.getByRole("heading", { name: "连接你的服务" }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("proxy node selection translates without changing the selected node", async ({ page }) => {
  await page.goto(`${base}/proxies`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await expect(page.getByRole("button", { name: "选择 Main / REJECT" })).toHaveAttribute("aria-pressed", "true");
  const mainGroup = page.locator("section.panel").filter({ has: page.getByRole("heading", { name: "Main", exact: true }) });
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByText("Node selections are saved per profile and restored after core and service restarts.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Refresh nodes" })).toBeVisible();
  await expect(mainGroup.getByText(/Selector · Current:/)).toBeVisible();
  await expect(page.getByRole("button", { name: "Select Main / REJECT" })).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("button", { name: "Select Main / DIRECT" }).click();
  await expect(page.getByRole("button", { name: "Select Main / DIRECT" })).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(mainGroup.getByText(/Selector · 当前：/)).toBeVisible();
  await expect(page.getByRole("button", { name: "选择 Main / DIRECT" })).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("button", { name: "选择 Main / REJECT" }).click();
  await expect(page.getByRole("button", { name: "选择 Main / REJECT" })).toHaveAttribute("aria-pressed", "true");
});

test("proxy delay controls translate without losing the test URL or results", async ({ page }) => {
  await page.goto(`${base}/proxies`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  const requests: Array<{ command: string; url: string }> = [];
  await page.route("**/api/commands", async (route) => {
    const body = route.request().postDataJSON();
    if (body?.command === "delay_proxy" || body?.command === "delay_group") {
      requests.push({ command: body.command, url: body.url });
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(body.command === "delay_proxy" ? { delay: 126 } : { DIRECT: 84, REJECT: 0 }),
      });
    } else {
      await route.continue();
    }
  });
  const url = "https://delay.example.test/204";
  await page.getByLabel("测速链接:").fill(url);
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByLabel("Delay test URL:")).toHaveValue(url);
  const group = page.locator("section.panel").filter({ has: page.getByRole("heading", { name: "Main", exact: true }) });
  const direct = group.getByRole("button", { name: "Select Main / DIRECT" });
  await group.locator('.node-test-btn[title="Test DIRECT latency"]').click();
  await expect(direct.locator(".delay-badge")).toHaveText("126ms");
  await group.getByRole("button", { name: "Test delay" }).click();
  await expect(direct.locator(".delay-badge")).toHaveText("84ms");
  await expect(group.getByRole("button", { name: "Select Main / REJECT" }).locator(".delay-badge")).toHaveText("Timed out");
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.getByLabel("测速链接:")).toHaveValue(url);
  await expect(group.getByRole("button", { name: "测速" })).toBeVisible();
  await expect(group.getByRole("button", { name: "选择 Main / REJECT" }).locator(".delay-badge")).toHaveText("超时");
  expect(requests).toEqual([{ command: "delay_proxy", url }, { command: "delay_group", url }]);
});

test.skip("proxy provider controls translate while an update is pending", async ({ page }) => {
  const calls: string[] = [];
  let releaseUpdate: () => void = () => {};
  const firstUpdate = new Promise<void>((resolve) => { releaseUpdate = resolve; });
  let holdFirstUpdate = true;
  await page.route("**/api/commands", async (route) => {
    const body = route.request().postDataJSON();
    if (body?.command === "proxy_providers") {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({
        providers: {
          FixtureProvider: { name: "FixtureProvider", type: "Proxy", vehicleType: "HTTP", proxies: [{ name: "Alpha" }, { name: "Beta" }] },
        },
      }) });
    } else if (body?.command === "update_proxy_provider" || body?.command === "healthcheck_proxy_provider") {
      calls.push(body.command);
      expect(body.name).toBe("FixtureProvider");
      if (body.command === "update_proxy_provider" && holdFirstUpdate) {
        holdFirstUpdate = false;
        await firstUpdate;
      }
      await route.fulfill({ status: 200, contentType: "application/json", body: "{}" });
    } else {
      await route.continue();
    }
  });
  await page.goto(`${base}/proxies`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  const card = page.locator(".provider-card").filter({ hasText: "FixtureProvider" });
  await expect(page.getByRole("heading", { name: "代理提供者 (Proxy Providers)" })).toBeVisible();
  await expect(card).toContainText("节点数：2");
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("heading", { name: "Proxy Providers" })).toBeVisible();
  await expect(page.getByText("External proxy collections: 1")).toBeVisible();
  await expect(card).toContainText("Nodes: 2");
  await card.getByRole("button", { name: "Update", exact: true }).click();
  await expect(card.getByRole("button", { name: "Updating…" })).toBeVisible();
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(card.getByRole("button", { name: "更新中…" })).toBeVisible();
  releaseUpdate();
  await expect(card.getByRole("button", { name: "更新", exact: true })).toBeEnabled();
  await card.getByRole("button", { name: "健康检查" }).click();
  await expect.poll(() => calls.length).toBe(2);
  await expect(card.getByRole("button", { name: "健康检查" })).toBeEnabled();
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await page.getByRole("button", { name: "Update all" }).click();
  await expect.poll(() => calls.length).toBe(3);
  await expect(card.getByRole("button", { name: "Update", exact: true })).toBeEnabled();
  expect(calls).toEqual(["update_proxy_provider", "healthcheck_proxy_provider", "update_proxy_provider"]);
});

test("rule list and search translate without losing the filter or reloading rules", async ({ page }) => {
  let ruleReads = 0;
  await page.route("**/api/commands", async (route) => {
    const body = route.request().postDataJSON();
    if (body?.command === "rules") {
      ruleReads += 1;
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ rules: [
        { type: "DOMAIN", payload: "example.test", proxy: "DIRECT" },
        { type: "IP-CIDR", payload: "10.0.0.0/8", proxy: "REJECT" },
      ] }) });
    } else if (body?.command === "rule_providers") {
      await route.fulfill({ status: 200, contentType: "application/json", body: '{"providers":{}}' });
    } else {
      await route.continue();
    }
  });
  await page.goto(base);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /规则/ }).click();
  const search = page.getByRole("searchbox", { name: "搜索规则" });
  await expect(page.getByRole("region", { name: "规则列表" })).toBeVisible();
  await expect(page.getByText("共 2 条", { exact: true })).toBeVisible();
  await search.fill("example");
  await expect(page.getByText("匹配 1 / 2 条")).toBeVisible();
  await expect(page.getByRole("row", { name: /example.test/ })).toBeVisible();
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("heading", { name: "Rules and routing policy" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Rule list" })).toBeVisible();
  await expect(page.getByRole("searchbox", { name: "Search rules" })).toHaveValue("example");
  await expect(page.getByText("1 of 2 matched")).toBeVisible();
  await expect(page.getByRole("columnheader", { name: "Target policy" })).toBeVisible();
  await page.getByRole("searchbox", { name: "Search rules" }).fill("absent.example");
  await expect(page.getByText("No matching rules found.")).toBeVisible();
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.getByRole("searchbox", { name: "搜索规则" })).toHaveValue("absent.example");
  await expect(page.getByText("没有找到匹配的规则。")).toBeVisible();
  await page.getByRole("button", { name: "清除" }).click();
  await expect(page.getByRole("row", { name: /example.test/ })).toBeVisible();
  await expect(page.getByRole("row", { name: /10.0.0.0\/8/ })).toBeVisible();
  expect(ruleReads).toBe(1);
});

test("rule provider inventory and update controls translate while an update is pending", async ({ page }) => {
  const calls: string[] = [];
  let releaseUpdate: () => void = () => {};
  const firstUpdate = new Promise<void>((resolve) => { releaseUpdate = resolve; });
  let holdFirstUpdate = true;
  await page.route("**/api/commands", async (route) => {
    const body = route.request().postDataJSON();
    if (body?.command === "rules") {
      await route.fulfill({ status: 200, contentType: "application/json", body: '{"rules":[]}' });
    } else if (body?.command === "rule_providers") {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({
        providers: {
          DirectRules: { name: "DirectRules", behavior: "domain", format: "text", ruleCount: 42, vehicleType: "HTTP", updatedAt: "2026-09-28 12:00" },
        },
      }) });
    } else if (body?.command === "update_rule_provider") {
      calls.push(body.command);
      expect(body.name).toBe("DirectRules");
      if (holdFirstUpdate) {
        holdFirstUpdate = false;
        await firstUpdate;
      }
      await route.fulfill({ status: 200, contentType: "application/json", body: "{}" });
    } else {
      await route.continue();
    }
  });
  await page.goto(base);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /规则/ }).click();
  const card = page.locator(".provider-card").filter({ hasText: "DirectRules" });
  await expect(page.getByRole("heading", { name: "外部规则集 (Rule Providers)" })).toBeVisible();
  await expect(card).toContainText("包含 42 条");
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("heading", { name: "Rule Providers" })).toBeVisible();
  await expect(card).toContainText("42 rules");
  await card.getByRole("button", { name: "Update", exact: true }).click();
  await expect(card.getByRole("button", { name: "Updating…" })).toBeVisible();
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(card.getByRole("button", { name: "更新中…" })).toBeVisible();
  releaseUpdate();
  await expect(card.getByRole("button", { name: "更新", exact: true })).toBeEnabled();
  await expect(page.getByText("更新成功：DirectRules")).toBeVisible();
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByText("Updated: DirectRules")).toBeVisible();
  await page.getByRole("button", { name: "Update all" }).click();
  await expect.poll(() => calls.length).toBe(2);
  await expect(card.getByRole("button", { name: "Update", exact: true })).toBeEnabled();
  expect(calls).toEqual(["update_rule_provider", "update_rule_provider"]);
});

test("logs page controls and empty/filter states translate across language changes", async ({ page }) => {
  await page.goto(`${base}/logs`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await expect(page.getByRole("heading", { name: "内核日志" })).toBeVisible();
  await expect(page.getByText("最近 200 条输出，实时更新。重连后重新读取日志尾部。")).toBeVisible();
  const filterInput = page.getByRole("textbox", { name: "筛选日志" });
  await expect(filterInput).toHaveAttribute("placeholder", "筛选日志…");
  await filterInput.fill("time=");
  await expect(page.getByRole("log")).toContainText("time=");
  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("heading", { name: "Core logs" })).toBeVisible();
  await expect(page.getByText("Recent 200 log entries, updated in real time. Re-fetches log tail upon reconnect.")).toBeVisible();
  const enFilterInput = page.getByRole("textbox", { name: "Filter logs" });
  await expect(enFilterInput).toHaveValue("time=");
  await expect(enFilterInput).toHaveAttribute("placeholder", "Filter logs…");
  await expect(page.getByRole("log")).toContainText("time=");
  await enFilterInput.fill("nonexistent_log_entry_pattern_xyz");
  await expect(page.getByText("No matching log entries found.")).toBeVisible();
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.getByText("没有找到匹配的日志。")).toBeVisible();
  await page.getByRole("button", { name: "清除" }).click();
  await expect(page.getByRole("log")).toContainText("time=");
});

test("core upgrade page controls and channels translate across language changes", async ({ page }) => {
  await page.route("**/api/commands", async (route) => {
    const body = route.request().postDataJSON();
    if (body?.command === "installed_core_version") {
      await route.fulfill({ json: "v1.18.0" });
      return;
    }
    if (body?.command === "core_installation") {
      await route.fulfill({ json: { version: "v1.18.0", stage_id: "init-stage" } });
      return;
    }
    if (body?.command === "core_release") {
      await route.fulfill({ json: { version: "v1.19.0", bytes: 1234567, target: "x86_64" } });
      return;
    }
    if (body?.command === "alpha_core_release") {
      await route.fulfill({ json: { version: "v1.19.0-alpha", bytes: 1234567, target: "x86_64" } });
      return;
    }
    await route.continue();
  });
  await page.goto(`${base}/core`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  await expect(page.getByRole("heading", { name: "稳定版内核升级" })).toBeVisible();
  await expect(page.getByRole("button", { name: "刷新安装信息" })).toBeVisible();
  await expect(page.getByLabel("升级通道")).toBeVisible();
  await expectHelp(page, "稳定版内核升级", "检查并安装 Mihomo 最新稳定版。升级时会短暂中断代理连接；失败时恢复上一份内核，停止的内核仍保持停止。");
  await expectHelp(page, "稳定版内核升级", "默认跳过相同版本；强制重新安装会重新验证并替换内核。");
  await expect(page.getByText("已验证安装 v1.18.0")).toBeVisible();
  await expect(page.getByRole("button", { name: "检查稳定版更新" })).toBeVisible();
  await expect(page.getByRole("button", { name: "升级至最新稳定版" })).toBeVisible();
  await expect(page.getByRole("button", { name: "强制重新安装稳定版" })).toBeVisible();

  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await expect(page.getByRole("heading", { name: "Stable core upgrade" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Refresh install info" })).toBeVisible();
  await expect(page.getByLabel("Upgrade channel")).toBeVisible();
  await expectHelp(page, "Stable core upgrade", "Check and install the latest Stable Mihomo core. Proxy connections will briefly pause during upgrades; previous core is restored on failure, stopped cores remain stopped.");
  await expect(page.getByText("Verified install v1.18.0")).toBeVisible();
  await expect(page.getByRole("button", { name: "Check Stable update" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Upgrade to latest Stable" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Force reinstall Stable" })).toBeVisible();

  await page.getByLabel("Upgrade channel").selectOption("alpha");
  await expect(page.getByRole("heading", { name: "Alpha core upgrade" })).toBeVisible();
  await expectHelp(page, "Alpha core upgrade", "Alpha is a pre-release channel. Switch back to Stable channel anytime.");
  await expectHelp(page, "Alpha core upgrade", "Check and install the latest Alpha Mihomo core. Proxy connections will briefly pause during upgrades; previous core is restored on failure, stopped cores remain stopped.");
  await expect(page.getByRole("button", { name: "Check Alpha update" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Upgrade to latest Alpha" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Force reinstall Alpha" })).toBeVisible();

  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.getByRole("heading", { name: "Alpha内核升级" })).toBeVisible();
  await expectHelp(page, "Alpha内核升级", "Alpha 是预发布版本。可选择稳定版通道切回最新稳定版。");
  await expectHelp(page, "Alpha内核升级", "检查并安装 Mihomo 最新Alpha。升级时会短暂中断代理连接；失败时恢复上一份内核，停止的内核仍保持停止。");
  await expect(page.getByRole("button", { name: "检查Alpha更新" })).toBeVisible();
  await expect(page.getByRole("button", { name: "升级至最新Alpha" })).toBeVisible();
  await expect(page.getByRole("button", { name: "强制重新安装Alpha" })).toBeVisible();

  await page.getByLabel("升级通道").selectOption("stable");
  await expect(page.getByRole("heading", { name: "稳定版内核升级" })).toBeVisible();
});

test("resources panel, geo seed and online actions translate across language changes", async ({ page }) => {
  await page.route("**/api/commands", async (route) => {
    const body = route.request().postDataJSON();
    if (body?.command === "resources") {
      await route.fulfill({
        json: {
          data_dir: "/mock/data",
          bundle_dir: "/mock/bundle",
          config_revision: "mock-rev",
          geo_update: {
            core_running: true,
            readback_error: false,
            configured_enabled: true,
            configured_interval_hours: 24,
            effective_enabled: true,
            effective_interval_hours: 24,
            auto_update_state: "active",
            mismatch: false,
          },
          geo: [
            {
              section: "geo",
              name: "Country.mmdb",
              state: "available",
              path: "Country.mmdb",
              provider_type: null,
              bytes: 4096,
              conflict: false,
              freshness: "fresh",
            },
          ],
          providers: [],
        },
      });
      return;
    }
    if (body?.command === "geo_seed") {
      await route.fulfill({
        json: {
          name: "Country.mmdb",
          current_sha256: "1111111111111111111111111111111111111111111111111111111111111111",
          seed_sha256: "2222222222222222222222222222222222222222222222222222222222222222",
          seed_bytes: 4096,
        },
      });
      return;
    }
    if (body?.command === "geo_online_info") {
      await route.fulfill({
        json: {
          name: "Country.mmdb",
          current_sha256: "1111111111111111111111111111111111111111111111111111111111111111",
          source_sha256: "3333333333333333333333333333333333333333333333333333333333333333",
        },
      });
      return;
    }
    await route.continue();
  });
  await page.goto(`${base}/settings`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  await openSettingsForEditing(page);

  await page.locator(".settings-details").filter({ has: page.locator("summary", { hasText: /内核实际设置|资源与数据库/ }) }).evaluateAll(nodes => nodes.forEach(node => (node as HTMLDetailsElement).open = true));
  const panel = page.getByRole("region", { name: "运行资源清单" });
  await expect(panel.getByRole("heading", { name: "Geo / Provider 资源" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "刷新资源清单" })).toBeVisible();
  await expect(panel.getByRole("heading", { name: "Geo 自动更新策略" })).toBeVisible();
  await expect(panel.getByText("活跃（自动更新中）")).toBeVisible();
  await expect(panel.getByText("文件存在 · 4096 字节")).toBeVisible();
  await expect(panel.getByRole("button", { name: "校验 Country.mmdb" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "读取 Country.mmdb 打包更新" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "读取 Country.mmdb 在线来源" })).toBeVisible();

  await panel.getByRole("button", { name: "读取 Country.mmdb 打包更新" }).click();
  await expect(panel.getByRole("checkbox", { name: "允许安装描述为空、完整结构未验证的 MMDB" })).toBeVisible();

  await panel.getByRole("button", { name: "读取 Country.mmdb 在线来源" }).click();
  await expect(panel.getByLabel("下载路由")).toBeVisible();
  await expect(panel.getByRole("checkbox", { name: "显式忽略下载来源证书错误" })).toBeVisible();

  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  const enPanel = page.getByRole("region", { name: "Runtime resource inventory" });
  await expect(enPanel.getByRole("heading", { name: "Geo / Provider resources" })).toBeVisible();
  await expect(enPanel.getByRole("button", { name: "Refresh resource list" })).toBeVisible();
  await expect(enPanel.getByRole("heading", { name: "Geo auto-update policy" })).toBeVisible();
  await expect(enPanel.getByText("Active (auto-updating)")).toBeVisible();
  await expect(enPanel.getByText("File exists · 4096 bytes")).toBeVisible();
  await expect(enPanel.getByRole("button", { name: "Validate Country.mmdb" })).toBeVisible();
  await expect(enPanel.getByRole("button", { name: "Read Country.mmdb bundle update" })).toBeVisible();
  await expect(enPanel.getByRole("button", { name: "Read Country.mmdb online source" })).toBeVisible();
  await expect(enPanel.getByLabel("Download route")).toBeVisible();
  await expect(enPanel.getByRole("checkbox", { name: "Explicitly ignore download source certificate errors" })).toBeVisible();

  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.getByRole("region", { name: "运行资源清单" })).toBeVisible();
});

test("manual remote refresh keeps identity, applies active config and preserves failures across restart", async ({
  page,
}) => {
  const api = async (path: string) => {
    const response = await fetch(`${base}/api/${path}`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  const remote = page
    .locator("article.profile")
    .filter({ hasText: "BrowserRemote.yaml" });
  await expect(remote).toBeVisible();
  const uid = await remote.locator(".mono").textContent();
  const before = await api("status");
  subscriptionUsage = "upload=1024; download=3072; total=8192; expire=0";
  const yaml =
    "mixed-port: 0\nproxies: []\nmode: rule\nprofile: {store-selected: false}\nproxy-groups: [{name: Main, type: select, proxies: [DIRECT, REJECT]}]\nrules: ['MATCH,Main']\n";
  subscriptionBody = yaml;
  await remote.getByRole("button", { name: /刷新订阅/ }).click();
  await expect(remote).toContainText("4.0 KB / 8.0 KB");
  expect((await api("status")).config_revision).toBe(before.config_revision);
  expect(await remote.locator(".mono").textContent()).toBe(uid);
  await remote.getByRole("button", { name: "使用订阅" }).click();
  await expect(remote).toContainText("当前订阅");
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /代理/ })
    .click();
  await page
    .getByRole("button", { name: "选择 Main / REJECT", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "选择 Main / REJECT", exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /订阅/ })
    .click();
  const selected = await api("status");
  subscriptionBody = yaml.replace("mode: rule", "mode: direct");
  await remote.getByRole("button", { name: /刷新订阅/ }).click();
  await expect
    .poll(async () => (await api("status")).config_revision)
    .not.toBe(selected.config_revision);
  const saved = await api("profiles");
  const status = await api("status");
  subscriptionBody = "proxies: []\nrules: ['INVALID,DIRECT']\n";
  await remote.getByRole("button", { name: /刷新订阅/ }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Mihomo rejected configuration",
  );
  expect(await api("profiles")).toEqual(saved);
  expect((await api("status")).config_revision).toBe(status.config_revision);
  await expect(page.locator(".sidebar-status")).toContainText("运行中");
  await expect(page.locator("article.profile")).toHaveCount(2);
  const requests = subscriptionRequests;
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  await expect(remote).toContainText("当前订阅");
  expect(await remote.locator(".mono").textContent()).toBe(uid);
  expect(await api("profiles")).toEqual(saved);
  expect(subscriptionRequests).toBe(requests);
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /代理/ })
    .click();
  await expect(
    page.getByRole("button", { name: "选择 Main / REJECT", exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /配置/ })
    .click();
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: direct/);
  await page.getByRole("button", { name: "退出登录" }).click();
});

test("profile metadata editing persists and deletion protects current profiles and removes saved content", async ({
  page,
}) => {
  const api = async (path: string) => {
    const response = await fetch(`${base}/api/${path}`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  const remote = page
    .locator("article.profile")
    .filter({ hasText: "BrowserRemote.yaml" });
  await expect(remote).toBeVisible();
  const uid = await remote.locator(".mono").textContent();
  const before = await api("status");
  const requests = subscriptionRequests;
  await remote
    .getByRole("button", { name: "操作 BrowserRemote.yaml", exact: true })
    .click();
  await remote
    .getByRole("button", { name: "编辑订阅 BrowserRemote.yaml", exact: true })
    .click();
  await page.getByLabel("修改订阅名称").fill("EditedRemote");
  await page
    .getByLabel("订阅描述", { exact: true })
    .fill("Saved browser description");
  await page
    .getByLabel("远程订阅链接", { exact: true })
    .fill(`${subscriptionUrl}/ok?edited=1`);
  await page.getByLabel("订阅 User-Agent").fill("browser-edit-agent");
  await page.getByLabel("下载超时（秒）").fill("5");
  await page.getByLabel("更新间隔（分钟）").fill("180");
  await page.getByLabel("允许自动更新", { exact: true }).uncheck();
  await page.getByRole("button", { name: "保存订阅信息", exact: true }).click();
  const edited = page
    .locator("article.profile")
    .filter({ hasText: "EditedRemote" });
  await expect(edited).toContainText("Saved browser description");
  expect(await edited.locator(".mono").textContent()).toBe(uid);
  expect((await api("status")).config_revision).toBe(before.config_revision);
  expect((await api("status")).pid).toBe(before.pid);
  expect(subscriptionRequests).toBe(requests);
  await edited
    .getByRole("button", { name: "操作 EditedRemote", exact: true })
    .click();
  await expect(
    edited.getByRole("button", { name: "删除订阅 EditedRemote", exact: true }),
  ).toBeDisabled();
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /内核/ }).click();
  await page.getByRole("button", { name: "停止内核", exact: true }).click();
  await expect(page.locator(".sidebar-status")).toContainText("已停止");
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /订阅/ }).click();
  await edited
    .getByRole("button", { name: "操作 EditedRemote", exact: true })
    .click();
  await expect(
    edited.getByRole("button", { name: "删除订阅 EditedRemote", exact: true }),
  ).toBeDisabled();
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /内核/ }).click();
  await page.getByRole("button", { name: "启动内核", exact: true }).click();
  await expect(page.locator(".sidebar-status")).toContainText("运行中");
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /订阅/ }).click();
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  await expect(edited).toContainText("当前订阅");
  const catalog = await api("profiles");
  const item = catalog.items.find((item: { uid: string }) => item.uid === uid);
  expect(item.desc).toBe("Saved browser description");
  expect(item.option.user_agent).toBe("browser-edit-agent");
  expect(item.option.timeout_seconds).toBe(5);
  expect(item.option.update_interval).toBe(180);
  expect(item.option.allow_auto_update).toBe(false);
  expect(item.url).toBe(`${subscriptionUrl}/ok?edited=1`);
  const local = page.locator("article.profile").filter({
    has: page.getByRole("heading", { name: "Browser", exact: true }),
  });
  await local
    .getByRole("button", { name: "操作 Browser", exact: true })
    .click();
  await page
    .getByRole("button", { name: "编辑订阅 Browser", exact: true })
    .click();
  await page
    .getByLabel("订阅描述", { exact: true })
    .fill("Local browser description");
  await expect(page.getByLabel("远程订阅链接", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "保存订阅信息", exact: true }).click();
  await expect(
    page.locator("article.profile").filter({
      has: page.getByRole("heading", { name: "Browser", exact: true }),
    }),
  ).toContainText("Local browser description");
  await local
    .getByRole("button", { name: "操作 Browser", exact: true })
    .click();
  await page
    .getByRole("button", { name: "删除订阅 Browser", exact: true })
    .click();
  await page.getByRole("button", { name: "取消删除", exact: true }).click();
  await expect(page.locator("article.profile")).toHaveCount(2);
  await local
    .getByRole("button", { name: "操作 Browser", exact: true })
    .click();
  await page
    .getByRole("button", { name: "删除订阅 Browser", exact: true })
    .click();
  await page
    .getByRole("button", { name: "确认删除 Browser", exact: true })
    .click();
  await expect(page.locator("article.profile")).toHaveCount(1);
  await expect(local).toHaveCount(0);
  const removed = catalog.items.find(
    (item: { name: string }) => item.name === "Browser",
  );
  await expect(
    readFile(join(directory, "profiles", removed.file)),
  ).rejects.toMatchObject({ code: "ENOENT" });
  await page.getByRole("button", { name: "+ 本地订阅", exact: true }).click();
  await page.getByLabel("订阅名称", { exact: true }).fill("Replacement");
  await page
    .getByLabel("订阅 YAML", { exact: true })
    .fill("proxies: []\nmode: direct\nmixed-port: 0\n");
  await page.getByRole("button", { name: "导入订阅", exact: true }).click();
  const replacement = page
    .locator("article.profile")
    .filter({ hasText: "Replacement" });
  await replacement
    .getByRole("button", { name: "使用订阅", exact: true })
    .click();
  await expect(replacement).toContainText("当前订阅");
  const current = await api("status");
  await edited
    .getByRole("button", { name: "操作 EditedRemote", exact: true })
    .click();
  await edited
    .getByRole("button", { name: "删除订阅 EditedRemote", exact: true })
    .click();
  await edited
    .getByRole("button", { name: "确认删除 EditedRemote", exact: true })
    .click();
  await expect(page.locator("article.profile")).toHaveCount(1);
  expect((await api("status")).config_revision).toBe(current.config_revision);
  await expect(
    readFile(join(directory, "profiles", item.file)),
  ).rejects.toMatchObject({ code: "ENOENT" });
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  await expect(replacement).toContainText("当前订阅");
  await expect(edited).toHaveCount(0);
  await page.setViewportSize({ width: 390, height: 844 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.getByRole("button", { name: "退出登录" }).click();
});

test("linked merge preserves raw subscriptions, rejects invalid updates and survives service restart", async ({
  page,
}) => {
  const api = async (path: string) => {
    const response = await fetch(`${base}/api/${path}`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  const navigate = async (name: RegExp) => {
    await page
      .getByRole("navigation", { name: "主导航" })
      .getByRole("link", { name })
      .click();
  };
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  const replacement = page.locator("article.profile").filter({
    has: page.getByRole("heading", { name: "Replacement", exact: true }),
  });
  await expect(replacement).toContainText("当前订阅");
  const uid = await replacement.locator(".mono").textContent();
  const catalog = await api("profiles");
  const item = catalog.items.find((item: { uid: string }) => item.uid === uid);
  const raw = await readFile(join(directory, "profiles", item.file), "utf8");
  const before = await api("status");
  await replacement
    .getByRole("button", { name: "操作 Replacement", exact: true })
    .click();
  await replacement
    .getByRole("button", { name: "合并增强 Replacement", exact: true })
    .click();
  const overlay = "# browser merge\nMODE: rule\n";
  await page.getByLabel("合并增强 YAML").fill(overlay);
  await page.getByRole("button", { name: "保存增强", exact: true }).click();
  await expect(page.getByLabel("合并增强 YAML")).toHaveCount(0);
  await expect(replacement).toContainText("已关联合并增强");
  await expect(page.locator("article.profile")).toHaveCount(1);
  const saved = await api("profiles");
  expect(saved.items).toHaveLength(8);
  const mergeUid = saved.items.find((item: { uid: string }) => item.uid === uid)
    .option.merge;
  const linked = saved.items.find(
    (item: { uid: string }) => item.uid === mergeUid,
  );
  expect(linked.uid).toMatch(/^m/);
  expect(
    saved.items.find((item: { uid: string }) => item.uid === uid).option.merge,
  ).toBe(linked.uid);
  expect(await readFile(join(directory, "profiles", linked.file), "utf8")).toBe(
    overlay,
  );
  expect(await readFile(join(directory, "profiles", item.file), "utf8")).toBe(
    raw,
  );
  expect((await api("status")).active_profile).toBe(uid);
  expect((await api("status")).config_revision).not.toBe(
    before.config_revision,
  );
  await navigate(/配置/);
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: rule/);
  await navigate(/订阅/);
  await replacement
    .getByRole("button", { name: "操作 Replacement", exact: true })
    .click();
  await replacement
    .getByRole("button", { name: "合并增强 Replacement", exact: true })
    .click();
  await expect(page.getByLabel("合并增强 YAML")).toHaveValue(overlay);
  const valid = await api("status");
  await page.getByLabel("合并增强 YAML").fill("rules: ['INVALID,Main']\n");
  await page.getByRole("button", { name: "保存增强", exact: true }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "保存增强", exact: true }),
  ).toBeEnabled();
  expect((await api("status")).config_revision).toBe(valid.config_revision);
  expect((await api("status")).pid).toBe(valid.pid);
  expect(await api("profiles")).toEqual(saved);
  await page.getByLabel("合并增强 YAML").fill("mode: global\n");
  await page.getByRole("button", { name: "保存增强", exact: true }).click();
  await expect(page.getByLabel("合并增强 YAML")).toHaveCount(0);
  expect((await api("profiles")).items).toHaveLength(8);
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  await expect(replacement).toContainText("已关联合并增强");
  await expect(page.locator(".sidebar-status")).toContainText("运行中");
  await navigate(/配置/);
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: global/);
  await navigate(/订阅/);
  await replacement
    .getByRole("button", { name: "操作 Replacement", exact: true })
    .click();
  await replacement
    .getByRole("button", { name: "合并增强 Replacement", exact: true })
    .click();
  await expect(page.getByLabel("合并增强 YAML")).toHaveValue("mode: global\n");
  await page.getByRole("button", { name: "移除增强", exact: true }).click();
  await expect(replacement.getByText("已关联合并增强")).toHaveCount(0);
  expect((await api("profiles")).items).toHaveLength(7);
  expect((await api("status")).active_profile).toBe(uid);
  expect(await readFile(join(directory, "profiles", item.file), "utf8")).toBe(
    raw,
  );
  await navigate(/配置/);
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: direct/);
  await expect(page.locator(".sidebar-status")).toContainText("运行中");
  await page.getByRole("button", { name: "退出登录" }).click();
});

test("linked sequence editor saves each type, rejects invalid rules and restores nodes across restart", async ({
  page,
}) => {
  const api = async (path: string) => {
    const response = await fetch(`${base}/api/${path}`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  const navigate = async (name: RegExp) => {
    await page
      .getByRole("navigation", { name: "主导航" })
      .getByRole("link", { name })
      .click();
  };
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  const profile = page.locator("article.profile").filter({
    has: page.getByRole("heading", { name: "Replacement", exact: true }),
  });
  await expect(profile).toContainText("当前订阅");
  const uid = await profile.locator(".mono").textContent();
  const catalog = await api("profiles");
  const baseItem = catalog.items.find(
    (item: { uid: string }) => item.uid === uid,
  );
  const raw = await readFile(
    join(directory, "profiles", baseItem.file),
    "utf8",
  );
  const empty = (kind: string) =>
    `# Profile Enhancement ${kind[0].toUpperCase()}${kind.slice(1)} Template for Clash Verge\n\nprepend: []\n\nappend: []\n\ndelete: []\n`;
  const rules =
    "# browser sequences\nprepend: ['DOMAIN,sequence.test,REJECT']\nappend: []\ndelete: []\n";
  for (const [kind, yaml] of [
    ["rules", rules],
    [
      "proxies",
      "prepend: [{name: BrowserAdded, type: direct}]\nappend: []\ndelete: []\n",
    ],
    [
      "groups",
      "prepend: [{name: SeqGroup, type: select, proxies: [BrowserAdded, DIRECT]}]\nappend: []\ndelete: []\n",
    ],
  ]) {
    await profile
      .getByRole("button", { name: "操作 Replacement", exact: true })
      .click();
    await profile
      .getByRole("button", { name: "序列增强 Replacement", exact: true })
      .click();
    if (kind !== "rules") {
      await page.getByLabel("序列增强类型").selectOption(kind);
      await expect(page.getByLabel("序列增强类型")).toHaveValue(kind);
    }
    await expect(page.getByLabel("序列增强 YAML")).toHaveValue(empty(kind));
    await page.getByLabel("序列增强 YAML").fill(yaml);
    await page
      .getByRole("button", { name: "保存序列增强", exact: true })
      .click();
    await expect(page.getByLabel("序列增强 YAML")).toHaveCount(0);
  }
  await expect(profile).toContainText("已关联序列增强");
  await expect(page.locator("article.profile")).toHaveCount(1);
  expect((await api("profiles")).items).toHaveLength(7);
  expect(
    await readFile(join(directory, "profiles", baseItem.file), "utf8"),
  ).toBe(raw);
  await navigate(/代理/);
  await page
    .getByRole("button", { name: "选择 SeqGroup / BrowserAdded", exact: true })
    .click();
  await expect(
    page.getByRole("button", {
      name: "选择 SeqGroup / BrowserAdded",
      exact: true,
    }),
  ).toHaveAttribute("aria-pressed", "true");
  await navigate(/订阅/);
  await profile
    .getByRole("button", { name: "操作 Replacement", exact: true })
    .click();
  await profile
    .getByRole("button", { name: "序列增强 Replacement", exact: true })
    .click();
  await expect(page.getByLabel("序列增强 YAML")).toHaveValue(rules);
  const saved = await api("profiles");
  const status = await api("status");
  await page
    .getByLabel("序列增强 YAML")
    .fill("prepend: ['INVALID,DIRECT']\nappend: []\ndelete: []\n");
  await page.getByRole("button", { name: "保存序列增强", exact: true }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "保存序列增强", exact: true }),
  ).toBeEnabled();
  expect(await api("profiles")).toEqual(saved);
  expect((await api("status")).config_revision).toBe(status.config_revision);
  expect((await api("status")).pid).toBe(status.pid);
  await page.getByRole("button", { name: "取消序列编辑", exact: true }).click();
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  await expect(profile).toContainText("已关联序列增强");
  await navigate(/代理/);
  await expect(
    page.getByRole("button", {
      name: "选择 SeqGroup / BrowserAdded",
      exact: true,
    }),
  ).toHaveAttribute("aria-pressed", "true");
  await navigate(/配置/);
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(
    /DOMAIN,sequence.test,REJECT/,
  );
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/SeqGroup/);
  await navigate(/订阅/);
  await page.setViewportSize({ width: 390, height: 844 });
  for (const kind of ["rules", "groups", "proxies"]) {
    await profile
      .getByRole("button", { name: "操作 Replacement", exact: true })
      .click();
    await profile
      .getByRole("button", { name: "序列增强 Replacement", exact: true })
      .click();
    if (kind !== "rules") {
      await page.getByLabel("序列增强类型").selectOption(kind);
      await expect(page.getByLabel("序列增强类型")).toHaveValue(kind);
    }
    await expect(
      page.getByRole("button", { name: "移除序列增强", exact: true }),
    ).toBeEnabled();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    await page
      .getByRole("button", { name: "移除序列增强", exact: true })
      .click();
    await expect(page.getByLabel("序列增强 YAML")).toHaveCount(0);
  }
  await expect(profile.getByText("已关联序列增强")).toHaveCount(0);
  // Removing the group triggers bounded asynchronous node-record reconciliation.
  // Finish that accepted operation before later tests snapshot the catalog.
  await expect
    .poll(async () => {
      const current = (await api("profiles")).items.find(
        (item: { uid: string }) => item.uid === uid,
      );
      return current.selected ?? [];
    })
    .toEqual([]);
  expect((await api("profiles")).items).toHaveLength(4);
  expect((await api("status")).active_profile).toBe(uid);
  expect(
    await readFile(join(directory, "profiles", baseItem.file), "utf8"),
  ).toBe(raw);
  await expect(page.locator(".sidebar-status")).toContainText("运行中");
  await page.getByRole("button", { name: "退出登录" }).click();
});

test("script editor validates failures, preserves raw content and restores after service restart", async ({
  page,
}) => {
  const api = async (path: string) => {
    const response = await fetch(`${base}/api/${path}`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  const navigate = async (name: RegExp) => {
    await page
      .getByRole("navigation", { name: "主导航" })
      .getByRole("link", { name })
      .click();
  };
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  const profile = page.locator("article.profile").filter({
    has: page.getByRole("heading", { name: "Replacement", exact: true }),
  });
  await expect(profile).toContainText("当前订阅");
  const uid = await profile.locator(".mono").textContent();
  const item = (await api("profiles")).items.find(
    (item: { uid: string }) => item.uid === uid,
  );
  const raw = await readFile(join(directory, "profiles", item.file), "utf8");
  const source =
    "// browser script\nfunction main(config, name) { console.info('BrowserScript ' + name); config.mode = 'rule'; return config; }\n";
  await profile
    .getByRole("button", { name: "操作 Replacement", exact: true })
    .click();
  await profile
    .getByRole("button", { name: "脚本增强 Replacement", exact: true })
    .click();
  await page.getByLabel("脚本增强 JavaScript").fill(source);
  await page.getByRole("button", { name: "保存脚本增强", exact: true }).click();
  await expect(page.getByLabel("脚本增强 JavaScript")).toHaveCount(0);
  await expect(profile).toContainText("已关联脚本增强");
  await expect(page.locator("article.profile")).toHaveCount(1);
  expect((await api("profiles")).items).toHaveLength(4);
  expect(await readFile(join(directory, "profiles", item.file), "utf8")).toBe(
    raw,
  );
  await navigate(/配置/);
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: rule/);
  await navigate(/日志/);
  await expect(
    page.getByText(/BrowserScript Replacement/).first(),
  ).toBeVisible();
  await navigate(/订阅/);
  await profile
    .getByRole("button", { name: "操作 Replacement", exact: true })
    .click();
  await profile
    .getByRole("button", { name: "脚本增强 Replacement", exact: true })
    .click();
  await expect(page.getByLabel("脚本增强 JavaScript")).toHaveValue(source);
  const saved = await api("profiles");
  const status = await api("status");
  for (const invalid of [
    "function main( {",
    "function main(config) { throw Error('browser failure'); }",
    "function main(config) { config.rules = ['INVALID,DIRECT']; return config; }",
  ]) {
    await page.getByLabel("脚本增强 JavaScript").fill(invalid);
    await page
      .getByRole("button", { name: "保存脚本增强", exact: true })
      .click();
    await expect(page.locator(".toast").last().getByRole("alert")).toBeVisible();
    await expect(
      page.getByRole("button", { name: "保存脚本增强", exact: true }),
    ).toBeEnabled();
    expect(await api("profiles")).toEqual(saved);
    expect((await api("status")).config_revision).toBe(status.config_revision);
    expect((await api("status")).pid).toBe(status.pid);
  }
  const updated =
    "function main(config) { config.mode = 'global'; return config; }\n";
  await page.getByLabel("脚本增强 JavaScript").fill(updated);
  await page.getByRole("button", { name: "保存脚本增强", exact: true }).click();
  await expect(page.getByLabel("脚本增强 JavaScript")).toHaveCount(0);
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  await expect(profile).toContainText("已关联脚本增强");
  await navigate(/配置/);
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: global/);
  await navigate(/订阅/);
  await profile
    .getByRole("button", { name: "操作 Replacement", exact: true })
    .click();
  await profile
    .getByRole("button", { name: "脚本增强 Replacement", exact: true })
    .click();
  await expect(page.getByLabel("脚本增强 JavaScript")).toHaveValue(updated);
  await page.setViewportSize({ width: 390, height: 844 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.getByRole("button", { name: "移除脚本增强", exact: true }).click();
  await expect(profile.getByText("已关联脚本增强")).toHaveCount(0);
  expect((await api("profiles")).items).toHaveLength(3);
  expect((await api("status")).active_profile).toBe(uid);
  expect(await readFile(join(directory, "profiles", item.file), "utf8")).toBe(
    raw,
  );
  await navigate(/配置/);
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: direct/);
  await expect(page.locator(".sidebar-status")).toContainText("运行中");
  await page.getByRole("button", { name: "退出登录" }).click();
});

test("global commands validate and persist edits, preserve failures and reset defaults across service restart", async ({
  page,
}) => {
  await page.goto(base);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible();
  const command = async (
    name: string,
    fields: Record<string, unknown> = {},
  ) => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command: name, ...fields }),
    });
    return { ok: response.ok, data: await response.json() };
  };
  const api = async (name: string) => {
    const response = await command(name);
    expect(response.ok).toBe(true);
    return response.data;
  };
  const initial = await api("profiles");
  const uid = (await api("status")).active_profile;
  const baseItem = initial.items.find(
    (item: { uid: string }) => item.uid === uid,
  );
  const originalRaw = await readFile(
    join(directory, "profiles", baseItem.file),
    "utf8",
  );
  const oldMerge = initial.items.find(
    (item: { uid: string }) => item.uid === "Merge",
  );
  const defaultMerge = (await api("global_merge")).yaml;
  const defaultScript = (await api("global_script")).source;
  const merge = "# global API workflow\nmode: global\n";
  const result = await command("set_global_merge", { yaml: merge });
  expect(result.ok).toBe(true);
  expect(result.data.uid).toBe("Merge");
  expect(result.data.file).not.toBe(oldMerge.file);
  expect(
    await readFile(join(directory, "profiles", oldMerge.file), "utf8"),
  ).toBe(defaultMerge);
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /配置/ })
    .click();
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: global/);
  const source =
    "function main(c,name) { console.info('BrowserGlobal '+name); c.mode='rule'; c['global-passes']=(c['global-passes']||0)+1; return c; }";
  expect((await command("set_global_script", { source })).ok).toBe(true);
  expect((await api("config")).yaml).toMatch(/global-passes: 2/);
  const saved = await api("profiles");
  const status = await api("status");
  for (const [name, fields] of [
    ["set_global_script", { source: "function main( {" }],
    [
      "set_global_script",
      { source: "function main(c) { throw 'global browser failure'; }" },
    ],
    ["set_global_merge", { yaml: "rules: ['INVALID,DIRECT']" }],
  ] as const) {
    expect((await command(name, fields)).ok).toBe(false);
    expect(await api("profiles")).toEqual(saved);
    expect((await api("status")).config_revision).toBe(status.config_revision);
    expect((await api("status")).pid).toBe(status.pid);
  }
  expect(
    await readFile(join(directory, "profiles", baseItem.file), "utf8"),
  ).toBe(originalRaw);
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  expect((await api("global_merge")).yaml).toBe(merge);
  expect((await api("global_script")).source).toBe(source);
  expect((await api("config")).yaml).toMatch(/global-passes: 2/);
  expect((await api("status")).active_profile).toBe(uid);
  expect(
    (await api("logs")).filter(
      (log: { stream: string }) => log.stream === "script",
    ),
  ).toHaveLength(0);
  expect((await command("reset_global_script")).ok).toBe(true);
  expect((await api("config")).yaml).toMatch(/mode: global/);
  expect((await command("reset_global_merge")).ok).toBe(true);
  expect((await api("global_merge")).yaml).toBe(defaultMerge);
  expect((await api("global_script")).source).toBe(defaultScript);
  expect((await api("profiles")).items).toHaveLength(3);
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /订阅/ })
    .click();
  await expect(page.locator("article.profile")).toHaveCount(1);
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /配置/ })
    .click();
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: direct/);
  await expect(page.locator(".sidebar-status")).toContainText("运行中");
  await page.getByRole("button", { name: "退出登录" }).click();
});

test("global editors save, retain failed drafts, confirm resets and work while stopped or without a profile", async ({
  page,
}) => {
  const api = async (name: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command: name, ...fields }),
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  const navigate = async (name: RegExp) => {
    await page
      .getByRole("navigation", { name: "主导航" })
      .getByRole("link", { name })
      .click();
  };
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  const global = page.getByRole("region", { name: "全局增强" });
  // A section has region semantics only with an accessible name.
  await expect(global).toBeVisible();
  await expect(global).toContainText("脚本可能执行两次");
  const catalog = await api("profiles");
  const uid = (await api("status")).active_profile;
  const baseItem = catalog.items.find(
    (item: { uid: string }) => item.uid === uid,
  );
  const raw = await readFile(
    join(directory, "profiles", baseItem.file),
    "utf8",
  );
  const defaultMerge = (await api("global_merge")).yaml;
  const defaultScript = (await api("global_script")).source;
  await global
    .getByRole("button", { name: "编辑全局合并", exact: true })
    .click();
  await expect(
    page.getByRole("textbox", { name: "全局合并 YAML", exact: true }),
  ).toHaveValue(defaultMerge);
  await page
    .getByRole("textbox", { name: "全局合并 YAML", exact: true })
    .fill("mode: rule");
  await page.getByRole("button", { name: "取消全局编辑", exact: true }).click();
  expect(await api("profiles")).toEqual(catalog);
  await global
    .getByRole("button", { name: "编辑全局合并", exact: true })
    .click();
  const merge = "# global editor YAML\nmode: global\n";
  await page
    .getByRole("textbox", { name: "全局合并 YAML", exact: true })
    .fill(merge);
  await page.getByRole("button", { name: "保存全局合并", exact: true }).click();
  await expect(
    page.getByRole("textbox", { name: "全局合并 YAML", exact: true }),
  ).toHaveCount(0);
  expect((await api("global_merge")).yaml).toBe(merge);
  await navigate(/配置/);
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: global/);
  await navigate(/订阅/);
  await global
    .getByRole("button", { name: "编辑全局脚本", exact: true })
    .click();
  await expect(
    page.getByRole("textbox", { name: "全局脚本 JavaScript", exact: true }),
  ).toHaveValue(defaultScript);
  const source =
    "function main(c,name) { console.info('GlobalEditor '+name); c.mode='rule'; c['editor-passes']=(c['editor-passes']||0)+1; return c; }";
  await page
    .getByRole("textbox", { name: "全局脚本 JavaScript", exact: true })
    .fill(source);
  await page.getByRole("button", { name: "保存全局脚本", exact: true }).click();
  await expect(
    page.getByRole("textbox", { name: "全局脚本 JavaScript", exact: true }),
  ).toHaveCount(0);
  expect((await api("config")).yaml).toMatch(/editor-passes: 2/);
  await navigate(/日志/);
  await expect(
    page.getByText(/GlobalEditor Replacement/).first(),
  ).toBeVisible();
  await navigate(/订阅/);
  await global
    .getByRole("button", { name: "编辑全局脚本", exact: true })
    .click();
  await expect(
    page.getByRole("textbox", { name: "全局脚本 JavaScript", exact: true }),
  ).toHaveValue(source);
  const saved = await api("profiles");
  const status = await api("status");
  for (const invalid of [
    "function main( {",
    "function main(c) { console.warn('editor failure'); throw 'bad global'; }",
    "function main(c) { c.rules=['INVALID,DIRECT']; return c; }",
  ]) {
    await page
      .getByRole("textbox", { name: "全局脚本 JavaScript", exact: true })
      .fill(invalid);
    await page
      .getByRole("button", { name: "保存全局脚本", exact: true })
      .click();
    await expect(page.getByRole("alert").first()).toBeVisible();
    await expect(
      page.getByRole("button", { name: "保存全局脚本", exact: true }),
    ).toBeEnabled();
    await expect(
      page.getByRole("textbox", { name: "全局脚本 JavaScript", exact: true }),
    ).toHaveValue(invalid);
    expect(await api("profiles")).toEqual(saved);
    expect((await api("status")).config_revision).toBe(status.config_revision);
    expect((await api("status")).pid).toBe(status.pid);
  }
  // Client size validation should not send an oversized worker request.
  let oversizedSent = 0;
  const inspect = async (route: import("@playwright/test").Route) => {
    if (route.request().postDataJSON()?.command === "set_global_script")
      oversizedSent += 1;
    await route.continue();
  };
  await page.route("**/api/commands", inspect);
  await page
    .getByRole("textbox", { name: "全局脚本 JavaScript", exact: true })
    .fill("x".repeat(1024 * 1024 + 1));
  await page.getByRole("button", { name: "保存全局脚本", exact: true }).click();
  await expect(
    page.getByText("脚本不能超过 1 MiB。", { exact: true }),
  ).toBeVisible();
  expect(oversizedSent).toBe(0);
  await page.unroute("**/api/commands", inspect);
  await page.getByRole("button", { name: "取消全局编辑", exact: true }).click();
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  await global
    .getByRole("button", { name: "编辑全局脚本", exact: true })
    .click();
  await expect(
    page.getByRole("textbox", { name: "全局脚本 JavaScript", exact: true }),
  ).toHaveValue(source);
  await page.setViewportSize({ width: 390, height: 844 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: "test-results/global-editor-mobile.png",
    fullPage: true,
  });
  await page
    .getByRole("button", { name: "恢复默认全局脚本", exact: true })
    .click();
  await expect(page.getByRole("group", { name: "恢复默认确认" })).toBeVisible();
  expect((await api("global_script")).source).toBe(source);
  await page.getByRole("button", { name: "继续编辑", exact: true }).click();
  await expect(page.getByRole("group", { name: "恢复默认确认" })).toHaveCount(
    0,
  );
  await page
    .getByRole("button", { name: "恢复默认全局脚本", exact: true })
    .click();
  await page.getByRole("button", { name: "确认恢复默认", exact: true }).click();
  await expect(
    page.getByRole("textbox", { name: "全局脚本 JavaScript", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /内核/ }).click();
  await page.getByRole("button", { name: "停止内核", exact: true }).click();
  await expect(page.locator(".sidebar-status")).toContainText("已停止");
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /订阅/ }).click();
  await global
    .getByRole("button", { name: "编辑全局合并", exact: true })
    .click();
  await expect(
    page.getByRole("textbox", { name: "全局合并 YAML", exact: true }),
  ).toHaveValue(merge);
  await page
    .getByRole("button", { name: "恢复默认全局合并", exact: true })
    .click();
  await page.getByRole("button", { name: "确认恢复默认", exact: true }).click();
  await expect(
    page.getByRole("textbox", { name: "全局合并 YAML", exact: true }),
  ).toHaveCount(0);
  expect((await api("global_merge")).yaml).toBe(defaultMerge);
  expect((await api("status")).pid).toBeNull();
  expect(
    await readFile(join(directory, "profiles", baseItem.file), "utf8"),
  ).toBe(raw);
  // Repair is still reachable when reading the saved global source fails.
  let rejectRead = true;
  const failRead = async (route: import("@playwright/test").Route) => {
    if (
      rejectRead &&
      route.request().postDataJSON()?.command === "global_script"
    ) {
      rejectRead = false;
      await route.fulfill({
        status: 503,
        contentType: "application/json",
        body: JSON.stringify({
          error: { message: "fixture global read failed" },
        }),
      });
    } else await route.continue();
  };
  await page.route("**/api/commands", failRead);
  await global
    .getByRole("button", { name: "编辑全局脚本", exact: true })
    .click();
  await expect(page.getByRole("alert").first()).toContainText(
    "fixture global read failed",
  );
  await expect(
    page.getByRole("button", { name: "保存全局脚本", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("button", { name: "重试读取全局增强", exact: true })
    .click();
  await expect(
    page.getByRole("textbox", { name: "全局脚本 JavaScript", exact: true }),
  ).toHaveValue(defaultScript);
  await page.unroute("**/api/commands", failRead);
  await page.getByRole("button", { name: "取消全局编辑", exact: true }).click();
  await api("apply_config", { yaml: "mode: direct\n" });
  await expect(global).toContainText("当前未选择订阅");
  await global
    .getByRole("button", { name: "编辑全局脚本", exact: true })
    .click();
  const deferred = "function main(c) { throw 'deferred UI execution'; }";
  await page
    .getByRole("textbox", { name: "全局脚本 JavaScript", exact: true })
    .fill(deferred);
  const standalone = await api("status");
  await page.getByRole("button", { name: "保存全局脚本", exact: true }).click();
  await expect(
    page.getByRole("textbox", { name: "全局脚本 JavaScript", exact: true }),
  ).toHaveCount(0);
  expect((await api("global_script")).source).toBe(deferred);
  expect((await api("status")).config_revision).toBe(
    standalone.config_revision,
  );
  await global
    .getByRole("button", { name: "编辑全局脚本", exact: true })
    .click();
  await page
    .getByRole("button", { name: "恢复默认全局脚本", exact: true })
    .click();
  await page.getByRole("button", { name: "确认恢复默认", exact: true }).click();
  await expect(
    page.getByRole("textbox", { name: "全局脚本 JavaScript", exact: true }),
  ).toHaveCount(0);
  await api("select_profile", { uid });
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /内核/ }).click();
  await page.getByRole("button", { name: "启动内核", exact: true }).click();
  await expect(page.locator(".sidebar-status")).toContainText("运行中");
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /订阅/ }).click();
  await expect(page.locator("article.profile")).toHaveCount(1);
  await page.getByRole("button", { name: "退出登录" }).click();
});

test("online settings commands apply to the browser runtime and retain stopped state across restart", async ({
  page,
}) => {
  const reopenConfig = async () => {
    const navigation = page.getByRole("navigation", { name: "主导航" });
    await navigation.getByRole("link", { name: /概览/ }).click();
    await navigation.getByRole("link", { name: /配置/ }).click();
  };
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command, ...fields }),
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await page.goto(`${base}/config`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  const prior = await api("status");
  expect((await api("settings")).runtime).toEqual({});
  const accepted = await api("set_settings", {
    runtime: { mode: "global", "mixed-port": 0, "allow-lan": false },
  });
  expect(accepted.runtime.mode).toBe("global");
  expect((await api("status")).active_profile).toBe(prior.active_profile);
  await reopenConfig();
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: global/);
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /内核/ }).click();
  await page.getByRole("button", { name: "停止内核", exact: true }).click();
  await expect(page.locator(".sidebar-status")).toContainText("已停止");
  await api("set_settings", { runtime: { mode: "rule", "mixed-port": 0 } });
  await reopenConfig();
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: rule/);
  expect((await api("status")).pid).toBeNull();
  await api("set_settings", { runtime: {} });
  await reopenConfig();
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: direct/);
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /内核/ }).click();
  await page.getByRole("button", { name: "启动内核", exact: true }).click();
  await expect(page.locator(".sidebar-status")).toContainText("运行中");
  await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: /配置/ }).click();
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  expect((await api("settings")).runtime).toEqual({});
  expect((await api("status")).active_profile).toBe(prior.active_profile);
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: direct/);
  await page.getByRole("button", { name: "退出登录" }).click();
});

test("settings save explicitly, preserve edits made during a save and reconcile uncertain saves", async ({ page }) => {
  const original = await settingsApi("settings");
  await settingsApi("stop");
  await settingsApi("set_settings", { runtime: {} });
  try {
    await loginSettings(page);
    const save = page.getByRole("button", { name: "保存服务设置", exact: true });
    await expect(save).toBeDisabled();
    for (const name of ["全部改为继承", "核对已保存设置"]) await expect(page.getByRole("button", { name, exact: true })).toHaveCount(0);
    const mode = page.getByRole("combobox", { name: "代理模式", exact: true });
    await mode.focus(); await mode.selectOption("global");
    expect((await settingsApi("settings")).runtime).toEqual({});
    await mode.blur();
    await expect(save).toBeEnabled();
    expect((await settingsApi("settings")).runtime).toEqual({});
    await applySettings(page);
    expect((await settingsApi("settings")).runtime).toEqual({ mode: "global" });
    let writes = 0, release!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    await page.route("**/api/commands", async route => {
      if (route.request().postDataJSON().command === "set_settings") { writes++; if (writes === 1) await held; }
      await route.continue();
    });
    await mode.focus(); await mode.selectOption("direct"); await save.click();
    await expect.poll(() => writes).toBe(1);
    const ipv6 = page.getByRole("combobox", { name: "IPv6", exact: true });
    await ipv6.focus(); await ipv6.selectOption("false");
    const port = page.getByRole("textbox", { name: "混合端口", exact: true });
    await port.fill("0"); // This input is still focused when the earlier response returns.
    release();
    await expect(save).toBeEnabled();
    await expect(ipv6).toHaveValue("false"); await expect(port).toHaveValue("0");
    expect((await settingsApi("settings")).runtime).toEqual({ mode: "direct" });
    await applySettings(page);
    expect((await settingsApi("settings")).runtime).toEqual({ mode: "direct", ipv6: false, "mixed-port": 0 });
    expect(writes).toBe(2);
    await page.unroute("**/api/commands");
    const saved = await settingsApi("settings");
    await editSetting(page, "混合端口", "70000");
    await expect(page.getByRole("region", { name: "服务设置编辑器" }).getByRole("alert")).toContainText("0–65535");
    expect(await settingsApi("settings")).toEqual(saved);
    await editSetting(page, "混合端口", "0");
    let failRead = true;
    await page.route("**/api/commands", async route => {
      if (failRead && route.request().postDataJSON().command === "settings") await route.fulfill({ status: 503, json: { error: { message: "fixture readback lost" } } });
      else await route.continue();
    });
    await editSetting(page, "统一延迟", "true", true);
    await expect(page.getByRole("region", { name: "服务设置编辑器" }).getByRole("alert")).toContainText("保存结果尚未核对");
    expect((await settingsApi("settings")).runtime["unified-delay"]).toBe(true);
    failRead = false;
    await editSetting(page, "统一延迟", "false", true);
    expect((await settingsApi("settings")).runtime["unified-delay"]).toBe(false);
    await page.unroute("**/api/commands");
    await page.route("**/api/commands", async route => {
      if (route.request().postDataJSON().command === "set_settings") {
        const response = await route.fetch(); expect(response.ok()).toBe(true);
        await route.fulfill({ status: 500, json: { error: { message: "fixture reply lost" } } });
      } else await route.continue();
    });
    await editSetting(page, "代理模式", "global", true);
    await expect(page.locator(".toast").filter({ hasText: "保存成功" }).last()).toBeVisible();
    expect((await settingsApi("settings")).runtime.mode).toBe("global");
    await page.unroute("**/api/commands");
    await editSetting(page, "代理模式", "", true);
    expect((await settingsApi("settings")).runtime.mode).toBeUndefined();
    expect((await settingsApi("status")).phase).toBe("stopped");
    await stop(); await start();
    expect((await settingsApi("settings")).runtime.mode).toBeUndefined();
  } finally { await page.unroute("**/api/commands"); await settingsApi("set_settings", { runtime: original.runtime }); }
});

test("main header stays sticky and settings navigation asks to save or discard", async ({ page }) => {
  const original = await settingsApi("settings");
  const initialMode = original.runtime.mode == null ? "" : String(original.runtime.mode);
  const alternateMode = initialMode === "global" ? "direct" : "global";
  try {
    await page.goto(`${base}/#token=${encodeURIComponent(token)}`);
    await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: "设置", exact: true }).click();
    await openSettingsForEditing(page);
    const header = page.locator(".page-header");
    await expect(header).toHaveCSS("position", "sticky");
    await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(0);
    await expect.poll(async () => Math.round((await header.boundingBox())!.y)).toBe(0);
    await page.evaluate(() => window.scrollTo(0, 0));

    const save = page.getByRole("button", { name: "保存服务设置", exact: true });
    await expect(save).toBeDisabled();
    await page.getByRole("combobox", { name: "代理模式", exact: true }).selectOption(alternateMode);
    await expect(save).toBeEnabled();
    await expect(page.getByText("有待应用的修改", { exact: true })).toHaveCount(0);
    expect((await settingsApi("settings")).runtime.mode ?? "").toBe(initialMode);

    await page.goBack();
    const dialog = page.getByRole("dialog", { name: "有未保存的设置" });
    await expect(dialog).toBeVisible();
    expect(new URL(page.url()).pathname).toBe("/settings");
    await dialog.getByRole("button", { name: "继续编辑", exact: true }).click();
    await expect(page.getByRole("combobox", { name: "代理模式", exact: true })).toHaveValue(alternateMode);

    await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: "概览", exact: true }).click();
    await dialog.getByRole("button", { name: "撤销改动", exact: true }).click();
    await expect(page.getByRole("heading", { name: "概览", exact: true })).toBeVisible();
    expect((await settingsApi("settings")).runtime.mode ?? "").toBe(initialMode);

    await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: "设置", exact: true }).click();
    await openSettingsForEditing(page);
    await page.getByRole("combobox", { name: "代理模式", exact: true }).selectOption(alternateMode);
    await page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: "概览", exact: true }).click();
    await page.getByRole("dialog", { name: "有未保存的设置" }).getByRole("button", { name: "保存并离开", exact: true }).click();
    await expect(page.getByRole("heading", { name: "概览", exact: true })).toBeVisible();
    expect((await settingsApi("settings")).runtime.mode).toBe(alternateMode);
  } finally {
    await settingsApi("set_settings", { runtime: original.runtime });
  }
});

test("nested network settings persist and scalar edits preserve them", async ({
  page,
}) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command, ...fields }),
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await api("stop");
  const runtime = {
    dns: { nameserver: ["1.1.1.1"], "enhanced-mode": "redir-host" },
    tun: { enable: false, "auto-route": false, mtu: 1500 },
  };
  const saved = await api("set_settings", { runtime });
  expect(saved.runtime).toEqual(runtime);
  const yaml = (await api("config")).yaml;
  expect(yaml).toMatch(/mtu: 1500/);
  expect(yaml).toContain("1.1.1.1");
  await stop();
  await start();
  expect(await api("settings")).toEqual(saved);
  await page.goto(`${base}/settings`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await openSettingsForEditing(page);
  await expect(
    page.getByRole("textbox", { name: "DNS 解析服务器", exact: true }),
  ).toHaveValue('["1.1.1.1"]');
  await page
    .getByRole("combobox", { name: "代理模式", exact: true })
    .selectOption("direct");
  await applySettings(page);
  await expect(
    page.getByText("保存成功", { exact: true }).last(),
  ).toBeVisible();
  expect((await api("settings")).runtime).toEqual({
    ...runtime,
    mode: "direct",
  });
  await api("set_settings", { runtime: {} });
  await page.getByRole("button", { name: "重新读取设置", exact: true }).click();
  await expect(
    page.getByRole("combobox", { name: "代理模式", exact: true }),
  ).toHaveValue("");
  await expect(
    page.getByRole("button", { name: "保存服务设置", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
});

test("provider DNS confirmation protects selection and expires on service restart", async ({
  page,
}) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command, ...fields }),
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await api("stop");
  await api("set_settings", {
    runtime: { dns: { nameserver: ["1.1.1.1"] }, tun: { enable: false } },
  });
  const profile = await api("import_profile", {
    name: "Browser DNS provider",
    yaml: "proxies: []\nmode: direct\nmixed-port: 0\ndns: {enable: false, nameserver: [9.9.9.9], nameserver-policy: {example.org: 8.8.8.8}}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']",
  });
  const uid = profile.uid;
  await api("select_profile", { uid });
  const state = await api("profile_dns", { uid });
  expect(state.enabled).toBe(false);
  expect(state.source).toMatch(/^[0-9a-f]{64}$/);
  expect((await api("config")).yaml).toContain("9.9.9.9");
  const before = (await api("status")).config_revision;
  for (const confirmation of [undefined, "0".repeat(64)]) {
    const result = await api("set_profile_dns", {
      uid,
      enabled: true,
      confirmation,
    });
    expect(result).toEqual({
      status: "confirmation_required",
      source: state.source,
    });
    expect((await api("status")).config_revision).toBe(before);
  }
  const result = await api("set_profile_dns", {
    uid,
    enabled: true,
    confirmation: state.source,
  });
  expect(result.status).toBe("applied");
  expect(result.state.enabled).toBe(true);
  const committed = (await api("config")).yaml;
  expect(committed).toContain("1.1.1.1");
  await stop();
  await start();
  expect((await api("profile_dns", { uid })).enabled).toBe(false);
  expect((await api("config")).yaml).toBe(committed);
  await api("select_profile", { uid });
  expect((await api("settings")).profile_dns[uid].enabled).toBe(false);
  expect((await api("config")).yaml).toContain("9.9.9.9");
  await page.goto(`${base}/settings`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  await openSettingsForEditing(page);
  await expect(
    page.getByRole("textbox", { name: "DNS 解析服务器", exact: true }),
  ).toHaveValue('["1.1.1.1"]');
  await api("set_settings", { runtime: {} });
  await page.getByRole("button", { name: "重新读取设置", exact: true }).click();
  await expect(page.getByRole("form", { name: "运行设置表单" })).toBeVisible();
  expect((await api("settings")).profile_dns[uid].enabled).toBe(false);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
});

test("network editor saves nested edits while preserving supported fields", async ({ page }) => {
  await settingsApi("stop");
  const runtime = {
    mode: "direct",
    "mixed-port": 0,
    dns: {
      enable: false,
      ipv6: false,
      "use-hosts": false,
      listen: "",
      "enhanced-mode": "redir-host",
      "fake-ip-filter-mode": "whitelist",
      "prefer-h3": true,
      "respect-rules": true,
      "fake-ip-range": "198.18.0.1/16",
      "fake-ip-range6": "2001:2::0/64",
      "default-nameserver": ["1.1.1.1"],
      nameserver: ["1.1.1.1"],
      fallback: [],
      "proxy-server-nameserver": ["8.8.8.8"],
      "direct-nameserver": ["9.9.9.9"],
      "nameserver-policy": { "owned.test": ["1.1.1.1"] },
      "fallback-filter": { geoip: false, "geoip-code": "CN" },
      "fake-ip-filter": ["*.lan"],
    },
    tun: {
      enable: false,
      stack: "mixed",
      device: "Mihomo",
      "auto-route": false,
      "auto-redirect": false,
      "auto-detect-interface": false,
      "strict-route": false,
      mtu: 1500,
      "route-exclude-address": [],
      "dns-hijack": [],
    },
  };
  const original = await settingsApi("settings");
  try {
    await settingsApi("set_settings", { runtime });
    await loginSettings(page);
    await editSetting(page, "TUN MTU", "1400");
    expect((await settingsApi("settings")).runtime).toEqual({ ...runtime, tun: { ...runtime.tun, mtu: 1400 } });
    const before = await settingsApi("settings");
    await editSetting(page, "DNS 解析服务器", '[1]');
    await expect(page.getByRole("region", { name: "服务设置编辑器" }).getByRole("alert")).toBeVisible();
    expect(await settingsApi("settings")).toEqual(before);
    await editSetting(page, "DNS 解析服务器", '["1.1.1.1"]');
    await editSetting(page, "DNS 启用", "false", true);
    expect((await settingsApi("settings")).runtime.dns.enable).toBe(false);
    await editSetting(page, "DNS 设置来源", "", true);
    expect((await settingsApi("settings")).runtime).toEqual({ mode: "direct", "mixed-port": 0, tun: { ...runtime.tun, mtu: 1400 } });
    await editSetting(page, "DNS 设置来源", "true", true);
    await editSetting(page, "DNS 解析服务器", '["1.1.1.1"]');
    await page.screenshot({ path: "test-results/network-settings-desktop.png", fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    await page.screenshot({ path: "test-results/network-settings-mobile.png", fullPage: true });
  } finally { await settingsApi("set_settings", { runtime: original.runtime }); }
});

test("final cleanup applies on remote refresh and settings authority without changing source YAML", async ({
  page,
}) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command, ...fields }),
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await api("stop");
  await api("set_settings", {
    runtime: { "mixed-port": 0, "allow-lan": true },
  });
  subscriptionBody =
    "proxies: []\nmode: rule\nmixed-port: 0\nallow-lan: false\nbind-address: localhost\nproxy-groups: [{name: choose, type: select, proxies: [missing-node, DIRECT]}]\nrules: ['MATCH,choose']\ndns: {enable: false}";
  const profile = await api("import_remote_profile", {
    url: subscriptionUrl,
    name: "Final cleanup provider",
  });
  const uid = profile.uid;
  await api("select_profile", { uid });
  const raw = await readFile(join(directory, "profiles", profile.file), "utf8");
  expect(raw).toBe(subscriptionBody);
  let yaml = (await api("config")).yaml;
  expect(yaml).not.toContain("missing-node");
  expect(yaml).toMatch(/bind-address: ['"]?\*['"]?/);
  expect(yaml.indexOf("allow-lan:")).toBeLessThan(
    yaml.indexOf("proxy-groups:"),
  );
  await page.goto(`${base}/settings`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  await openSettingsForEditing(page);
  await page
    .getByRole("combobox", { name: "允许局域网访问", exact: true })
    .selectOption("false");
  await applySettings(page);
  await expect(
    page.getByText("保存成功", { exact: true }).last(),
  ).toBeVisible();
  expect((await api("config")).yaml).toContain("bind-address: localhost");
  subscriptionBody = subscriptionBody
    .replace("missing-node", "new-missing-node")
    .replace("bind-address: localhost", "bind-address: '[::1]'");
  const refreshed = await api("refresh_profile", { uid });
  expect(
    await readFile(join(directory, "profiles", refreshed.file), "utf8"),
  ).toBe(subscriptionBody);
  yaml = (await api("config")).yaml;
  expect(yaml).not.toContain("new-missing-node");
  expect(yaml).toContain("[::1]");
  await page
    .getByRole("combobox", { name: "允许局域网访问", exact: true })
    .selectOption("true");
  await applySettings(page);
  await expect(
    page.getByText("保存成功", { exact: true }).last(),
  ).toBeVisible();
  yaml = (await api("config")).yaml;
  expect(yaml).toMatch(/bind-address: ['"]?\*['"]?/);
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /配置/ })
    .click();
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(yaml);
  await api("start");
  expect((await api("proxies")).proxies.choose.all).toEqual(["DIRECT"]);
  const committed = (await api("config")).yaml;
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  expect((await api("config")).yaml).toBe(committed);
  expect((await api("proxies")).proxies.choose.all).toEqual(["DIRECT"]);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
});

test("raw subscription editor preserves failed drafts, reconciles lost responses and detects refresh conflicts", async ({
  page,
}) => {
  test.setTimeout(90000);
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command, ...fields }),
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await api("stop");
  await api("set_settings", { runtime: { "mixed-port": 0 } });
  subscriptionBody =
    "# provider original\nproxies: []\nmode: direct\nmixed-port: 0\ndns: {enable: false}\nrules: ['MATCH,DIRECT']\n";
  const profile = await api("import_remote_profile", {
    url: subscriptionUrl,
    name: "Raw editor provider",
  });
  const uid = profile.uid;
  await api("select_profile", { uid });
  await api("set_profile_script", {
    uid,
    source:
      "function main(c) { if(c.mode==='global')c.rules=['INVALID,DIRECT']; c['raw-mode']=c.mode;return c; }",
  });
  const original = await api("profile_raw", { uid });
  const metadata = (await api("profiles")).items.find(
    (item: { uid: string }) => item.uid === uid,
  );
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  const failRead = async (route: import("@playwright/test").Route) => {
    if (route.request().postDataJSON()?.command === "profile_raw")
      await route.fulfill({
        status: 503,
        contentType: "application/json",
        body: JSON.stringify({ error: { message: "fixture raw read failed" } }),
      });
    else await route.continue();
  };
  await page.route("**/api/commands", failRead);
  await page
    .getByRole("button", {
      name: "操作 Raw editor provider",
      exact: true,
    })
    .click();
  await page
    .getByRole("button", {
      name: "编辑原始订阅 Raw editor provider",
      exact: true,
    })
    .click();
  const editor = page.getByRole("region", {
    name: "原始订阅编辑器",
    exact: true,
  });
  const input = editor.getByRole("textbox", {
    name: "原始订阅 YAML",
    exact: true,
  });
  const save = editor.getByRole("button", {
    name: "保存原始订阅",
    exact: true,
  });
  await expect(editor.getByRole("alert")).toContainText(
    "fixture raw read failed",
  );
  await expect(input).toHaveCount(0);
  await page.unroute("**/api/commands", failRead);
  await editor
    .getByRole("button", { name: "重试读取原始订阅", exact: true })
    .click();
  await expect(input).toHaveValue(original.yaml);
  await expect(save).toBeDisabled();
  const prior = await api("status");
  for (const invalid of [
    "proxies: [",
    original.yaml.replace("MATCH,DIRECT", "INVALID,DIRECT"),
    original.yaml.replace("mode: direct", "mode: global"),
  ]) {
    await input.fill(invalid);
    await save.click();
    await expect(
      page.getByText(
        "保存失败：服务内容不一致，请重新读取。",
        { exact: true },
      ).last(),
    ).toBeVisible();
    await expect(input).toHaveValue(invalid);
    expect(await api("profile_raw", { uid })).toEqual(original);
    expect((await api("status")).config_revision).toBe(prior.config_revision);
    expect((await api("status")).pid).toBe(prior.pid);
  }
  let draft = original.yaml
    .replace("# provider original", "# manual edit")
    .replace("mode: direct", "mode: rule");
  await input.fill(draft);
  await save.click();
  await expect(
    page.getByText("保存成功", { exact: true }).last(),
  ).toBeVisible();
  expect((await api("profile_raw", { uid })).yaml).toBe(draft);
  let nextMetadata = (await api("profiles")).items.find(
    (item: { uid: string }) => item.uid === uid,
  );
  expect({ ...nextMetadata, file: metadata.file }).toEqual(metadata);
  expect((await api("config")).yaml).toContain("raw-mode: rule");
  expect((await api("status")).pid).toBeNull();
  // A mutation applied before a lost reply must not be submitted twice.
  const loseReply = async (route: import("@playwright/test").Route) => {
    if (route.request().postDataJSON()?.command === "set_profile_raw") {
      await route.fetch();
      await route.abort("failed");
    } else await route.continue();
  };
  await page.route("**/api/commands", loseReply);
  draft = draft.replace("# manual edit", "# lost reply");
  await input.fill(draft);
  await save.click();
  await expect(page.locator(".toast-loading")).toHaveCount(0);
  await expect(
    page.getByText("保存成功", { exact: true }).last(),
  ).toBeVisible();
  await expect(save).toBeDisabled();
  expect((await api("profile_raw", { uid })).yaml).toBe(draft);
  await page.unroute("**/api/commands", loseReply);
  // Save/readback failure retains the draft and blocks another mutation.
  await page.route("**/api/commands", failRead);
  draft = draft.replace("# lost reply", "# uncertain save");
  await input.fill(draft);
  await save.click();
  await expect(editor.getByRole("alert").first()).toContainText(
    "保存结果尚未核对",
  );
  await expect(input).toHaveValue(draft);
  await expect(save).toBeDisabled();
  await page.unroute("**/api/commands", failRead);
  await editor
    .getByRole("button", { name: "核对原始订阅", exact: true })
    .click();
  await expect(
    page.getByText("内容一致", { exact: true }).last(),
  ).toBeVisible();
  // Refresh changes the immutable source revision; checking does not adopt it
  // as the editing base and silently overwrite the external change.
  const unsaved = draft.replace("# uncertain save", "# unsaved local draft");
  await input.fill(unsaved);
  subscriptionBody = subscriptionBody.replace(
    "# provider original",
    "# provider refreshed",
  );
  await api("refresh_profile", { uid });
  await expect(editor.getByRole("alert").last()).toContainText(
    "原始订阅版本已变化",
  );
  await expect(save).toBeDisabled();
  await expect(input).toHaveValue(unsaved);
  await editor
    .getByRole("button", { name: "核对原始订阅", exact: true })
    .click();
  await expect(save).toBeDisabled();
  await expect(input).toHaveValue(unsaved);
  await editor
    .getByRole("button", { name: "重新读取原始订阅", exact: true })
    .click();
  await expect(
    editor.getByRole("group", { name: "原始订阅草稿替换确认" }),
  ).toBeVisible();
  await editor
    .getByRole("button", { name: "继续编辑原始订阅", exact: true })
    .click();
  await expect(input).toHaveValue(unsaved);
  await editor
    .getByRole("button", { name: "重新读取原始订阅", exact: true })
    .click();
  await editor
    .getByRole("button", { name: "确认重新读取原始订阅", exact: true })
    .click();
  await expect(input).toHaveValue(subscriptionBody);
  await expect(save).toBeDisabled();
  // Keep editing the same UID after switching the active subscription.
  const other = await api("import_profile", {
    name: "Raw editor other",
    yaml: subscriptionBody,
  });
  await api("select_profile", { uid: other.uid });
  await expect(editor).toContainText("下次使用时生成增强配置");
  const before = await api("status");
  draft = subscriptionBody.replace("# provider refreshed", "# inactive saved");
  await input.fill(draft);
  await save.click();
  await expect(
    page.getByText("保存成功", { exact: true }).last(),
  ).toBeVisible();
  expect((await api("status")).config_revision).toBe(before.config_revision);
  expect((await api("profile_raw", { uid })).yaml).toBe(draft);
  await editor.screenshot({ path: "test-results/raw-editor-desktop.png" });
  await page.setViewportSize({ width: 390, height: 844 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await editor.screenshot({ path: "test-results/raw-editor-mobile.png" });
  await input.fill(draft + "# dirty\n");
  await editor
    .getByRole("button", { name: "关闭原始编辑器", exact: true })
    .click();
  await editor
    .getByRole("button", { name: "继续编辑原始订阅", exact: true })
    .click();
  await expect(input).toHaveValue(draft + "# dirty\n");
  await editor
    .getByRole("button", { name: "关闭原始编辑器", exact: true })
    .click();
  await editor
    .getByRole("button", { name: "确认丢弃原始草稿", exact: true })
    .click();
  await expect(editor).toHaveCount(0);
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  expect((await api("profile_raw", { uid })).yaml).toBe(draft);
  await page
    .getByRole("button", {
      name: "操作 Raw editor provider",
      exact: true,
    })
    .click();
  await page
    .getByRole("button", {
      name: "编辑原始订阅 Raw editor provider",
      exact: true,
    })
    .click();
  await expect(input).toHaveValue(draft);
  await editor
    .getByRole("button", { name: "关闭原始编辑器", exact: true })
    .click();
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
});

test("proxy connection information follows actual ports, settings saves and stopped cores", async ({
  page,
}) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command, ...fields }),
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  const freePort = async () => {
    const listener = createServer();
    await new Promise<void>((resolve) =>
      listener.listen(0, "127.0.0.1", resolve),
    );
    const address = listener.address();
    if (!address || typeof address === "string")
      throw new Error("No test port");
    await new Promise<void>((resolve) => listener.close(() => resolve()));
    return address.port;
  };
  const first = await freePort();
  const second = await freePort();
  await api("set_settings", { runtime: {} });
  const profile = await api("import_profile", {
    name: "HTTP connection test",
    yaml: `port: ${first}\nmode: direct\nallow-lan: false\ndns: {enable: false}\ntun: {enable: false}\n`,
  });
  await api("select_profile", { uid: profile.uid });
  await api("start");
  await page.goto(base);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  const panel = page.getByRole("region", { name: "代理连接信息", exact: true });
  const httpRow = panel.getByRole("row").filter({
    has: page.getByRole("rowheader", { name: "HTTP", exact: true }),
  });
  await expect(httpRow).toContainText(String(first));
  await expect(httpRow.getByRole("cell").nth(1)).toHaveText(String(first));
  await expect(
    panel.getByText(`127.0.0.1:${first}`, { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /设置/ })
    .click();
  await expect(panel).toHaveCount(0);
  const input = page.getByRole("textbox", { name: "HTTP 端口", exact: true });
  const portHelp = page.locator("#port-hint-port");
  await portHelp.hover();
  const portHint = page.getByRole("tooltip");
  await expect(input).toHaveValue("");
  await expect(input).toHaveAttribute("placeholder", `继承当前端口 ${first}`);
  await portHelp.hover();
  await expect(portHint).toHaveText(`当前端口：${first} · 继承订阅 / 配置`);
  await page.locator("#port-hint-mixed-port").hover();
  await expect(portHint).toContainText("禁用（0）");
  await portHelp.hover();
  await input.fill(String(second));
  // A draft must not overwrite the displayed runtime or imply it is already live.
  await portHelp.hover();
  await expect(portHint).toContainText(`当前端口：${first}`);
  expect((await api("settings")).runtime).toEqual({});
  await input.blur();
  expect((await api("settings")).runtime).toEqual({});
  await applySettings(page);
  await expect(input).toHaveValue(String(second));
  await portHelp.hover();
  await expect(portHint).toHaveText(`当前端口：${second} · 服务设置`);
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /概览/ })
    .click();
  await expect(
    panel.getByText(`127.0.0.1:${second}`, { exact: true }),
  ).toBeVisible();
  const snapshot = await api("proxy_access");
  const failPort = async (route: import("@playwright/test").Route) => {
    if (route.request().postDataJSON()?.command === "proxy_access") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          ...snapshot,
          ports: snapshot.ports.map((port: { key: string }) =>
            port.key === "port" ? { ...port, actual: 0 } : port,
          ),
        }),
      });
    } else await route.continue();
  };
  await page.route("**/api/commands", failPort);
  await panel.getByRole("button", { name: "刷新连接信息" }).click();
  await expect(panel.getByRole("alert")).toContainText(
    "配置端口与内核实际端口不一致",
  );
  await expect(httpRow.getByRole("cell").nth(1)).toHaveText("未监听");
  await expect(
    panel.getByText(`127.0.0.1:${second}`, { exact: true }),
  ).toHaveCount(0);
  await page.unroute("**/api/commands", failPort);
  await api("stop");
  await expect(panel).toContainText("内核未运行");
  await expect(httpRow.getByRole("cell").nth(0)).toHaveText(String(second));
  await expect(httpRow.getByRole("cell").nth(1)).toHaveText("未确认");
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /设置/ })
    .click();
  await expect(panel).toHaveCount(0);
  await portHelp.hover();
  await expect(portHint).toHaveText(
    `配置端口：${second} · 服务设置 · 内核未运行`,
  );
  await expect(input).toHaveValue(String(second));
  await api("start");
  await portHelp.hover();
  await expect(portHint).toHaveText(`当前端口：${second} · 服务设置`);
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /概览/ })
    .click();
  await expect(
    panel.getByText(`127.0.0.1:${second}`, { exact: true }),
  ).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(panel).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.getByRole("button", { name: "退出登录" }).click();
});

test("deleting a linked subscription cascades auxiliaries and DNS preferences while preserving the running profile", async ({
  page,
}) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command, ...fields }),
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  const catalog = async () => {
    const response = await fetch(`${base}/api/profiles`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  const status = async () => {
    const response = await fetch(`${base}/api/status`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await api("set_settings", { runtime: { "mixed-port": 0, port: 0 } });
  const yaml =
    "mode: direct\nmixed-port: 0\ndns: {enable: false}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']";
  const deleted = await api("import_profile", {
    name: "Cascade deletion",
    yaml,
  });
  const retained = await api("import_profile", {
    name: "Cascade retained",
    yaml,
  });
  await api("select_profile", { uid: deleted.uid });
  await api("set_profile_dns", { uid: deleted.uid, enabled: false });
  await api("set_profile_merge", { uid: deleted.uid, yaml: "mode: direct" });
  await api("set_profile_script", {
    uid: deleted.uid,
    source: "function main(c) { return c; }",
  });
  for (const kind of ["rules", "proxies", "groups"]) {
    await api("set_profile_sequence", {
      uid: deleted.uid,
      kind,
      yaml: "prepend: []\nappend: []\ndelete: []",
    });
  }
  const previous = await catalog();
  const baseItem = previous.items.find(
    (item: { uid: string }) => item.uid === deleted.uid,
  );
  const auxiliaries = ["merge", "script", "rules", "proxies", "groups"].map(
    (key) => baseItem.option[key],
  );
  const removed = previous.items.filter(
    (item: { uid: string }) =>
      item.uid === deleted.uid || auxiliaries.includes(item.uid),
  );
  expect(removed).toHaveLength(6);
  await api("select_profile", { uid: retained.uid });
  await api("set_profile_dns", { uid: retained.uid, enabled: false });
  const before = await status();
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  const card = page.locator("article.profile").filter({
    has: page.getByRole("heading", { name: "Cascade deletion", exact: true }),
  });
  await card
    .getByRole("button", { name: "操作 Cascade deletion", exact: true })
    .click();
  await card
    .getByRole("button", { name: "删除订阅 Cascade deletion", exact: true })
    .click();
  await expect(card).toContainText("共享辅助配置会保留");
  await card
    .getByRole("button", { name: "确认删除 Cascade deletion", exact: true })
    .click();
  await expect(card).toHaveCount(0);
  const after = await catalog();
  for (const item of removed) {
    expect(
      after.items.some((row: { uid: string }) => row.uid === item.uid),
    ).toBe(false);
    await expect(
      readFile(join(directory, "profiles", item.file)),
    ).rejects.toMatchObject({ code: "ENOENT" });
  }
  const settings = await api("settings");
  expect(settings.profile_dns[deleted.uid]).toBeUndefined();
  expect(settings.profile_dns[retained.uid]).toEqual({ enabled: false });
  expect(
    after.items.some((item: { uid: string }) => item.uid === "Merge"),
  ).toBe(true);
  expect(
    after.items.some((item: { uid: string }) => item.uid === "Script"),
  ).toBe(true);
  expect((await status()).pid).toBe(before.pid);
  expect((await status()).config_revision).toBe(before.config_revision);
  const retainedCard = page.locator("article.profile").filter({
    has: page.getByRole("heading", { name: "Cascade retained", exact: true }),
  });
  await retainedCard
    .getByRole("button", { name: "操作 Cascade retained", exact: true })
    .click();
  await expect(
    page.getByRole("button", {
      name: "删除订阅 Cascade retained",
      exact: true,
    }),
  ).toBeDisabled();
  await retainedCard
    .getByRole("button", { name: "操作 Cascade retained", exact: true })
    .click();
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  await expect(card).toHaveCount(0);
  expect((await api("settings")).profile_dns[deleted.uid]).toBeUndefined();
  await retainedCard
    .getByRole("button", { name: "操作 Cascade retained", exact: true })
    .click();
  await expect(
    page.getByRole("button", {
      name: "删除订阅 Cascade retained",
      exact: true,
    }),
  ).toBeDisabled();
  await retainedCard
    .getByRole("button", { name: "操作 Cascade retained", exact: true })
    .click();
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
});

test("local and remote imports create owned defaults, preserve links across refresh and survive restart", async ({
  page,
}) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command, ...fields }),
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  const before = await api("status");
  const raw =
    "# automatic browser import\nmode: direct\nmixed-port: 0\ndns: {enable: false}\nrules: ['MATCH,DIRECT']";
  const local = await api("import_profile", {
    name: "Automatic defaults",
    yaml: raw,
  });
  const remote = await api("import_remote_profile", {
    name: "Automatic remote defaults",
    url: `${subscriptionUrl}/ok?defaults=1`,
  });
  const snapshot = await api("profiles");
  const owned = async (baseItem: {
    uid: string;
    option: Record<string, string>;
  }) => {
    const ids: string[] = [];
    for (const kind of ["merge", "script", "rules", "proxies", "groups"]) {
      const uid = baseItem.option[kind];
      expect(typeof uid).toBe("string");
      ids.push(uid);
      expect(["Merge", "Script", "Rules", "Proxies", "Groups"]).not.toContain(
        uid,
      );
      const item = snapshot.items.find(
        (row: { uid: string }) => row.uid === uid,
      );
      expect(item.type).toBe(kind);
      const source = await readFile(
        join(directory, "profiles", item.file),
        "utf8",
      );
      if (kind === "merge") expect(source).not.toContain("store-selected");
      else if (kind === "script") expect(source).toContain("return config;");
      else
        for (const key of ["prepend", "append", "delete"])
          expect(source).toContain(`${key}: []`);
    }
    return ids;
  };
  const localIds = await owned(local);
  const remoteIds = await owned(remote);
  expect(localIds.every((uid) => !remoteIds.includes(uid))).toBe(true);
  expect((await api("profile_raw", { uid: local.uid })).yaml).toBe(raw);
  expect((await api("status")).pid).toBe(before.pid);
  expect((await api("status")).config_revision).toBe(before.config_revision);
  await api("refresh_profile", { uid: remote.uid });
  expect(
    (await api("profiles")).items.find(
      (row: { uid: string }) => row.uid === remote.uid,
    ).option,
  ).toEqual(remote.option);
  const card = page.locator("article.profile").filter({
    has: page.getByRole("heading", {
      name: "Automatic defaults",
      exact: true,
    }),
  });
  await expect(card).toBeVisible();
  await card
    .getByRole("button", { name: "操作 Automatic defaults", exact: true })
    .click();
  await card
    .getByRole("button", { name: "合并增强 Automatic defaults", exact: true })
    .click();
  await expect(page.getByLabel("合并增强 YAML")).toHaveValue(
    "# Profile Enhancement Merge Template for Clash Verge\n\n",
  );
  await page.getByRole("button", { name: "取消增强编辑", exact: true }).click();
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  const restored = await api("profiles");
  for (const item of [local, remote])
    expect(
      restored.items.find((row: { uid: string }) => row.uid === item.uid)
        .option,
    ).toEqual(item.option);
  expect((await api("profile_raw", { uid: local.uid })).yaml).toBe(raw);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
});

test("managed proxy downloads persist the mode, keep failed drafts and allow explicit direct refresh", async ({
  page,
}) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command, ...fields }),
    });
    if (!response.ok) {
      console.error(`API command ${command} failed (${response.status}):`, await response.text());
    }
    expect(response.ok).toBe(true);
    return response.json();
  };
  const listener = createServer();
  await new Promise<void>((resolve) =>
    listener.listen(0, "127.0.0.1", resolve),
  );
  const port = (listener.address() as { port: number }).port;
  await new Promise<void>((resolve, reject) =>
    listener.close((error) => (error ? reject(error) : resolve())),
  );
  await api("set_settings", {
    runtime: { "mixed-port": port, port: 0, mode: "direct" },
  });
  await api("start");
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  await page.getByRole("button", { name: "+ 远程订阅", exact: true }).click();
  const url = `${subscriptionUrl}/ok?self_proxy=1`;
  await page.getByLabel("订阅链接", { exact: true }).fill(url);
  await page.getByLabel("远程订阅名称（可选）").fill("Managed download");
  await page.getByLabel("通过托管内核代理下载", { exact: true }).check();
  await page.getByRole("button", { name: "下载并导入", exact: true }).click();
  const card = page.locator("article.profile").filter({
    has: page.getByRole("heading", { name: "Managed download", exact: true }),
  });
  await expect(card).toBeVisible();
  const item = (await api("profiles")).items.find(
    (p: { name: string }) => p.name === "Managed download",
  );
  expect(item.option.self_proxy).toBe(true);
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  expect(
    (await api("profiles")).items.find(
      (p: { uid: string }) => p.uid === item.uid,
    ).option.self_proxy,
  ).toBe(true);
  await api("stop");
  const requests = subscriptionRequests;
  await page.getByRole("button", { name: "+ 远程订阅", exact: true }).click();
  await page.getByLabel("订阅链接", { exact: true }).fill(url);
  await page.getByLabel("远程订阅名称（可选）").fill("Keep proxy draft");
  await page.getByRole("button", { name: "下载并导入", exact: true }).click();
  await expect(
    page.getByText(/self_proxy requires a running managed core/),
  ).toBeVisible();
  await expect(page.getByLabel("订阅链接", { exact: true })).toHaveValue(url);
  await expect(page.getByLabel("远程订阅名称（可选）")).toHaveValue(
    "Keep proxy draft",
  );
  await expect(
    page.getByLabel("通过托管内核代理下载", { exact: true }),
  ).toBeChecked();
  expect(subscriptionRequests).toBe(requests);
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await card
    .getByRole("button", { name: "操作 Managed download", exact: true })
    .click();
  await card
    .getByRole("button", { name: "编辑订阅 Managed download", exact: true })
    .click();
  await page.getByLabel("订阅刷新通过托管内核代理", { exact: true }).uncheck();
  await page.getByRole("button", { name: "保存订阅信息", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "保存订阅信息", exact: true }),
  ).toHaveCount(0);
  expect(
    (await api("profiles")).items.find(
      (p: { uid: string }) => p.uid === item.uid,
    ).option.self_proxy,
  ).toBe(false);
  await api("refresh_profile", { uid: item.uid });
  expect(subscriptionRequests).toBe(requests + 1);
  await page.getByRole("button", { name: "+ 远程订阅", exact: true }).click();
  await page.getByLabel("通过托管内核代理下载", { exact: true }).uncheck();
  await page.getByRole("button", { name: "下载并导入", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Keep proxy draft", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
});

test("service system proxy import and refresh work while the core is stopped and persist explicit mode changes", async ({
  page,
}) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command, ...fields }),
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await api("stop");
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  await page.getByRole("button", { name: "+ 远程订阅", exact: true }).click();
  const url = "http://browser-subscription.invalid/ok?system=1";
  await page.getByLabel("订阅链接", { exact: true }).fill(url);
  await page.getByLabel("远程订阅名称（可选）").fill("System download");
  await page.getByLabel("使用服务系统代理下载", { exact: true }).check();
  await page.getByRole("button", { name: "下载并导入", exact: true }).click();
  const card = page.locator("article.profile").filter({
    has: page.getByRole("heading", { name: "System download", exact: true }),
  });
  await expect(card).toBeVisible();
  const item = (await api("profiles")).items.find(
    (p: { name: string }) => p.name === "System download",
  );
  expect(item.option.with_proxy).toBe(true);
  expect(item.option.self_proxy).toBe(false);
  const count = subscriptionRequests;
  await api("refresh_profile", { uid: item.uid });
  expect(subscriptionRequests).toBe(count + 1);
  expect((await api("status")).phase).toBe("stopped");
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  expect(
    (await api("profiles")).items.find(
      (p: { uid: string }) => p.uid === item.uid,
    ).option.with_proxy,
  ).toBe(true);
  await api("stop");
  await card
    .getByRole("button", { name: "操作 System download", exact: true })
    .click();
  await card
    .getByRole("button", { name: "编辑订阅 System download", exact: true })
    .click();
  await expect(
    page.getByLabel("订阅刷新使用服务系统代理", { exact: true }),
  ).toBeChecked();
  await page.getByLabel("订阅刷新使用服务系统代理", { exact: true }).uncheck();
  await page
    .getByLabel("远程订阅链接", { exact: true })
    .fill(`${subscriptionUrl}/ok?direct=1`);
  await page.getByRole("button", { name: "保存订阅信息", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "保存订阅信息", exact: true }),
  ).toHaveCount(0);
  expect(
    (await api("profiles")).items.find(
      (p: { uid: string }) => p.uid === item.uid,
    ).option.with_proxy,
  ).toBe(false);
  await api("refresh_profile", { uid: item.uid });
  const requests = subscriptionRequests;
  await page.getByRole("button", { name: "+ 远程订阅", exact: true }).click();
  await page.getByLabel("订阅链接", { exact: true }).fill(url);
  await page.getByLabel("远程订阅名称（可选）").fill("Keep system draft");
  await page.getByLabel("通过托管内核代理下载", { exact: true }).check();
  await page.getByRole("button", { name: "下载并导入", exact: true }).click();
  await expect(
    page.getByText(/self_proxy requires a running managed core/),
  ).toBeVisible();
  expect(subscriptionRequests).toBe(requests);
  await expect(page.getByLabel("订阅链接", { exact: true })).toHaveValue(url);
  await expect(
    page.getByLabel("使用服务系统代理下载", { exact: true }),
  ).toBeChecked();
  await page.getByLabel("通过托管内核代理下载", { exact: true }).uncheck();
  await page.getByRole("button", { name: "下载并导入", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Keep system draft", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
});

test("HTTPS certificate option is explicit, keeps failed drafts and persists across restart and refresh", async ({
  page,
}) => {
  const request = async (
    command: string,
    fields: Record<string, unknown> = {},
  ) =>
    fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command, ...fields }),
    });
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await request(command, fields);
    expect(response.ok).toBe(true);
    return response.json();
  };
  await api("stop");
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  await page.getByRole("button", { name: "+ 远程订阅", exact: true }).click();
  await expect(
    page.getByLabel("下载允许无效 TLS 证书", { exact: true }),
  ).not.toBeChecked();
  await page.getByLabel("订阅链接", { exact: true }).fill(tlsSubscriptionUrl);
  await page.getByLabel("远程订阅名称（可选）").fill("TLS subscription");
  await page.getByRole("button", { name: "下载并导入", exact: true }).click();
  await expect(
    page.getByText(/static webpki roots fallback failed/),
  ).toBeVisible();
  await expect(page.getByLabel("订阅链接", { exact: true })).toHaveValue(
    tlsSubscriptionUrl,
  );
  await expect(page.getByLabel("远程订阅名称（可选）")).toHaveValue(
    "TLS subscription",
  );
  expect(tlsSubscriptionRequests).toBe(0);
  await page.getByLabel("下载允许无效 TLS 证书", { exact: true }).check();
  await page.getByRole("button", { name: "下载并导入", exact: true }).click();
  const card = page.locator("article.profile").filter({
    has: page.getByRole("heading", { name: "TLS subscription", exact: true }),
  });
  await expect(card).toBeVisible();
  const item = (await api("profiles")).items.find(
    (p: { name: string }) => p.name === "TLS subscription",
  );
  expect(item.option.danger_accept_invalid_certs).toBe(true);
  expect(tlsSubscriptionRequests).toBe(1);
  const raw = await readFile(join(directory, "profiles", item.file));
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  await api("stop");
  await card
    .getByRole("button", { name: "操作 TLS subscription", exact: true })
    .click();
  await card
    .getByRole("button", { name: "编辑订阅 TLS subscription", exact: true })
    .click();
  await expect(
    page.getByLabel("订阅刷新允许无效 TLS 证书", { exact: true }),
  ).toBeChecked();
  await page.getByLabel("订阅刷新允许无效 TLS 证书", { exact: true }).uncheck();
  await page.getByRole("button", { name: "保存订阅信息", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "保存订阅信息", exact: true }),
  ).toHaveCount(0);
  const saved = (await api("profiles")).items.find(
    (p: { uid: string }) => p.uid === item.uid,
  );
  expect(saved.option.danger_accept_invalid_certs).toBe(false);
  for (const key of ["merge", "script", "rules", "proxies", "groups"])
    expect(saved.option[key]).toBe(item.option[key]);
  expect((await request("refresh_profile", { uid: item.uid })).ok).toBe(false);
  expect(tlsSubscriptionRequests).toBe(1);
  expect(await readFile(join(directory, "profiles", item.file))).toEqual(raw);
  await api("edit_profile", {
    uid: item.uid,
    patch: { options: { danger_accept_invalid_certs: true } },
  });
  await api("refresh_profile", { uid: item.uid });
  expect(tlsSubscriptionRequests).toBe(2);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
});

test("saved automatic update policy runs overdue subscriptions after restart and can be disabled in the editor", async ({
  page,
}) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ command, ...fields }),
    });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await api("stop");
  subscriptionBody = "proxies: []\nmode: rule\n";
  const item = await api("import_remote_profile", {
    url: `${subscriptionUrl}/scheduled`,
    name: "Scheduled subscription",
    options: { update_interval: 1, allow_auto_update: false },
  });
  const catalog = await api("profiles");
  // Only this isolated fixture is edited, while its service is stopped. JSON is
  // valid YAML; the HTTP metadata API deliberately does not accept updated timestamps.
  for (const row of catalog.items)
    if (row.type === "remote") row.option.allow_auto_update = false;
  const scheduled = catalog.items.find(
    (row: { uid: string }) => row.uid === item.uid,
  );
  scheduled.option.allow_auto_update = true;
  scheduled.updated = 1;
  await stop();
  await writeFile(join(directory, "profiles.yaml"), JSON.stringify(catalog));
  const requests = subscriptionRequests;
  subscriptionBody = "proxies: []\nmode: direct\n";
  await start();
  await expect
    .poll(
      async () =>
        (await api("profiles")).items.find(
          (row: { uid: string }) => row.uid === item.uid,
        ).file,
    )
    .not.toBe(item.file);
  expect(subscriptionRequests).toBe(requests + 1);
  expect(
    await readFile(
      join(
        directory,
        "profiles",
        (await api("profiles")).items.find(
          (row: { uid: string }) => row.uid === item.uid,
        ).file,
      ),
      "utf8",
    ),
  ).toContain("mode: direct");
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  const card = page.locator("article.profile").filter({
    has: page.getByRole("heading", {
      name: "Scheduled subscription",
      exact: true,
    }),
  });
  await card
    .getByRole("button", {
      name: "操作 Scheduled subscription",
      exact: true,
    })
    .click();
  await card
    .getByRole("button", {
      name: "编辑订阅 Scheduled subscription",
      exact: true,
    })
    .click();
  await expect(page.getByLabel("更新间隔（分钟）")).toHaveValue("1");
  await expect(page.getByLabel("允许自动更新", { exact: true })).toBeChecked();
  await page.getByLabel("允许自动更新", { exact: true }).uncheck();
  await page.getByRole("button", { name: "保存订阅信息", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "保存订阅信息", exact: true }),
  ).toHaveCount(0);
  expect(
    (await api("profiles")).items.find(
      (row: { uid: string }) => row.uid === item.uid,
    ).option.allow_auto_update,
  ).toBe(false);
  await api("refresh_profile", { uid: item.uid });
  expect(subscriptionRequests).toBe(requests + 2);
  expect(
    (await api("logs")).some(
      (line: { stream: string; message: string }) =>
        line.stream === "scheduler" && line.message.includes("completed"),
    ),
  ).toBe(true);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
});


// The optional binary comes from the verified official-package smoke check. Local
// metadata fixtures keep browser tests deterministic; public wrapper discovery,
// no-op, force and repair are covered by the separate real-node check.
async function installBrowserCore(api: (command: string, fields?: Record<string, unknown>) => Promise<any>, alpha: boolean) {
  const binary = alpha ? process.env.MIHOMO_TEST_ALPHA_BINARY! : join(resolve(process.env.MIHOMO_TEST_BUNDLE!), "resources/core/verge-mihomo");
  const version = execFileSync(binary, ["-v"], { encoding: "utf8" }).split(/\s+/)[2];
  expect(version).toMatch(alpha ? /^alpha-[a-f0-9]{7,40}$/ : /^v[0-9]+\.[0-9]+\.[0-9]+$/);
  const bytes = gzipSync(await readFile(binary), { level: 1 });
  const sha256 = createHash("sha256").update(bytes).digest("hex");
  const id = `${version}-${sha256}`;
  const cache = join(directory, "core/.upgrade-staging", id);
  await mkdir(cache, { recursive: true, mode: 0o700 });
  await chmod(cache, 0o700);
  await writeFile(join(cache, "package.gz"), bytes, { mode: 0o600 });
  await writeFile(join(cache, "release.json"), JSON.stringify({ schema_version: 1, release: {
    version, target: "x86_64-unknown-linux-gnu", asset: `mihomo-linux-amd64-v2-${version}.gz`,
    bytes: bytes.length, sha256,
    download_url: `https://github.com/MetaCubeX/mihomo/releases/download/${alpha ? "Prerelease-Alpha" : version}/mihomo-linux-amd64-v2-${version}.gz`,
  } }), { mode: 0o600 });
  const staged = await api("stage_core_upgrade", { id });
  await api("activate_core_upgrade", { id: staged.stage_id });
}

for (const channel of ["stable", "alpha"] as const) {
  const alpha = channel === "alpha";
  const label = alpha ? "Alpha" : "稳定版";
  test(`${channel} upgrade controls preserve no-op, failures and activation receipts after restart`, async ({
    page,
  }) => {
    test.skip(
      !process.env.MIHOMO_TEST_BUNDLE,
      "Requires the managed-core bundle",
    );
    const api = async (command: string, fields: Record<string, unknown> = {}) => {
      const response = await fetch(`${base}/api/commands`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ command, ...fields }),
      });
      expect(response.ok).toBe(true);
      return response.json();
    };
    test.skip(alpha && !process.env.MIHOMO_TEST_ALPHA_BINARY, "Requires a verified Alpha binary");
    if (alpha) await installBrowserCore(api, true);
    await api("start");
    const before = await api("status");
    const version = await api("installed_core_version");
    const binary = join(directory, "core/verge-mihomo");
    const inode = (await stat(binary)).ino;
    const packageBytes = gzipSync(await readFile(binary), { level: 1 });
    const sha256 = createHash("sha256").update(packageBytes).digest("hex");
    const id = `${version}-${sha256}`;
    const cache = join(directory, "core/.upgrade-staging", id);
    await mkdir(cache, { recursive: true, mode: 0o700 });
    await writeFile(join(cache, "package.gz"), packageBytes, { mode: 0o600 });
    await writeFile(
      join(cache, "release.json"),
      JSON.stringify({
        schema_version: 1,
        release: {
          version,
          target: "x86_64-unknown-linux-gnu",
          asset: `mihomo-linux-amd64-v2-${version}.gz`,
          bytes: packageBytes.length,
          sha256,
          download_url: `https://github.com/MetaCubeX/mihomo/releases/download/${alpha ? "Prerelease-Alpha" : version}/mihomo-linux-amd64-v2-${version}.gz`,
        },
      }),
      { mode: 0o600 },
    );
    await chmod(cache, 0o700);
    let calls = 0,
      fail = true;
    let release!: () => void;
    const hold = new Promise<void>((resolve) => {
      release = resolve;
    });
    // The browser gets fixture official-discovery/results; forced success delegates
    // to real authenticated staging/activation. Rust and separate official smoke
    // tests exercise the actual wrapper's metadata/download/no-op decisions.
    await page.route("**/api/commands", async (route) => {
      const body = route.request().postDataJSON();
      if (body.command === (alpha ? "alpha_core_release" : "core_release")) {
        await route.fulfill({
          json: {
            version,
            bytes: packageBytes.length,
            target: "x86_64-unknown-linux-gnu",
          },
        });
      } else if (body.command === (alpha ? "upgrade_alpha_core" : "upgrade_clash_core")) {
        calls += 1;
        expect(Object.keys(body).sort()).toEqual(["command", "force"]);
        if (!body.force) {
          await hold;
          await route.fulfill({
            json: { upgraded: false, from: version, to: version },
          });
        } else if (fail) {
          await route.fulfill({
            status: 422,
            json: {
              error: {
                message: "fixture upgrade failed; previous core restored",
              },
            },
          });
        } else {
          const staged = await api("stage_core_upgrade", { id });
          const activated = await api("activate_core_upgrade", {
            id: staged.stage_id,
          });
          await route.fulfill({
            json: {
              upgraded: activated.upgraded,
              from: activated.from,
              to: activated.to,
            },
          });
        }
      } else await route.continue();
    });
    await page.goto(`${base}/core`);
    await page.getByLabel("管理令牌").fill(token);
    await page.getByRole("button", { name: "连接服务", exact: true }).click();
    if (alpha) await page.getByLabel("升级通道").selectOption("alpha");
    const panel = page.getByRole("region", { name: `${label}内核升级` });
    await expect(
      panel.getByRole("button", { name: `升级至最新${label}`, exact: true }),
    ).toBeEnabled();
    await panel
      .getByRole("button", { name: `检查${label}更新`, exact: true })
      .click();
    await expect(panel.locator("dd").nth(1)).toHaveText(version);
    await panel
      .getByRole("button", { name: `升级至最新${label}`, exact: true })
      .click();
    await expect(
      panel.getByRole("button", { name: `升级至最新${label}`, exact: true }),
    ).toBeDisabled();
    await expect(page.getByLabel("升级通道")).toBeDisabled();
    release();
    await expect(
      page.getByText(`已是最新${label} ${version}`, {
        exact: true,
      }),
    ).toBeVisible();
    await page.locator(".toast").filter({ hasText: `已是最新${label} ${version}` }).getByRole("button").click();
    await page.getByLabel("升级通道").selectOption(alpha ? "stable" : "alpha");
    await expect(page.getByText(`已是最新${label} ${version}`, { exact: true })).toHaveCount(0);
    await page.getByLabel("升级通道").selectOption(channel);
    await expect(panel.locator("dd").nth(1)).toHaveText("尚未检查");
    expect((await api("status")).pid).toBe(before.pid);
    expect((await stat(binary)).ino).toBe(inode);
    const forced = panel.getByRole("button", {
      name: `强制重新安装${label}`,
      exact: true,
    });
    await expect(forced).toBeEnabled();
    page.once("dialog", (dialog) => dialog.dismiss());
    await forced.click();
    expect(calls).toBe(1);
    page.once("dialog", (dialog) => dialog.accept());
    await forced.click();
    await expect(
      page.getByText("fixture upgrade failed; previous core restored", {
        exact: true,
      }),
    ).toBeVisible();
    await expect(
      page.getByText(`已是最新${label} ${version}`, {
        exact: true,
      }),
    ).toHaveCount(0);
    await expect(forced).toBeEnabled();
    fail = false;
    page.once("dialog", (dialog) => dialog.accept());
    await forced.click();
    await expect(
      page.getByText(`升级成功：${version}`, { exact: true }).last(),
    ).toBeVisible();
    await expect(
      panel.getByText(`已验证安装 ${version}`, { exact: true }),
    ).toBeVisible();
    expect((await api("status")).pid).not.toBe(before.pid);
    expect((await stat(binary)).ino).not.toBe(inode);
    await api("stop");
    page.once("dialog", (dialog) => dialog.accept());
    await expect(forced).toBeEnabled();
    await forced.click();
    await expect.poll(async () => (await api("status")).phase).toBe("stopped");
    await expect(
      page.getByText(`升级成功：${version}`, { exact: true }).last(),
    ).toBeVisible();
    await page.getByRole("button", { name: "退出登录", exact: true }).click();
    await stop();
    await start();
    await page.goto(`${base}/core`);
    await page.getByLabel("管理令牌").fill(token);
    await page.getByRole("button", { name: "连接服务", exact: true }).click();
    if (alpha) await page.getByLabel("升级通道").selectOption("alpha");
    await expect(
      panel.getByText(`已验证安装 ${version}`, { exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: "退出登录", exact: true }).click();
    expect(errors).toEqual([]);
  });
}

for (const channel of ["stable", "alpha"] as const) {
  const alpha = channel === "alpha";
  const label = alpha ? "Alpha" : "稳定版";
  test(`${channel} broken core repair preserves originals and reads receipts after reconnect`, async ({ page }) => {
    test.skip(!process.env.MIHOMO_TEST_BUNDLE, "Requires the managed-core bundle");
    const api = async (command: string, fields: Record<string, unknown> = {}) => {
      const response = await fetch(`${base}/api/commands`, {
        method: "POST",
        headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
        body: JSON.stringify({ command, ...fields }),
      });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error.message);
      return result;
    };
    test.skip(alpha && !process.env.MIHOMO_TEST_ALPHA_BINARY, "Requires a verified Alpha binary");
    await installBrowserCore(api, alpha);
    await api("stop");
    const version: string = await api("installed_core_version");
    const binary = join(directory, "core/verge-mihomo");
    const workingBytes = await readFile(binary);
    const seed = async (bytes: Buffer) => {
      const packageBytes = gzipSync(bytes, { level: 1 });
      const sha256 = createHash("sha256").update(packageBytes).digest("hex");
      const id = `${version}-${sha256}`;
      const cache = join(directory, "core/.upgrade-staging", id);
      await mkdir(cache, { recursive: true, mode: 0o700 });
      await chmod(cache, 0o700);
      await writeFile(join(cache, "package.gz"), packageBytes, { mode: 0o600 });
      await writeFile(join(cache, "release.json"), JSON.stringify({ schema_version: 1, release: {
        version, target: "x86_64-unknown-linux-gnu", asset: `mihomo-linux-amd64-v2-${version}.gz`,
        bytes: packageBytes.length, sha256,
        download_url: `https://github.com/MetaCubeX/mihomo/releases/download/${alpha ? "Prerelease-Alpha" : version}/mihomo-linux-amd64-v2-${version}.gz`,
      } }), { mode: 0o600 });
      return id;
    };
    const good = await seed(workingBytes);
    const source = join(directory, "failed-repair.rs");
    const bad = join(directory, "failed-repair");
    await writeFile(source, `fn main(){let a:Vec<_>=std::env::args().collect();if a.get(1).map(String::as_str)==Some("-v"){println!("Mihomo Meta ${version} linux amd64");return;}if a.get(1).map(String::as_str)==Some("-t"){return;}std::process::exit(1);}`);
    execFileSync("rustc", ["--crate-name", "repair_fixture", source, "-o", bad], { stdio: "ignore" });
    const failed = await seed(await readFile(bad));
    await stop();
    await writeFile(binary, Buffer.alloc(0));
    await chmod(binary, 0);
    const inode = (await stat(binary)).ino;
    await start();
    let fail = true;
    await page.route("**/api/commands", async (route) => {
      const body = route.request().postDataJSON();
      if (body.command !== (alpha ? "upgrade_alpha_core" : "upgrade_clash_core")) { await route.continue(); return; }
      expect(body.force).toBe(false);
      // Discovery is supplied locally; staging, activation, rollback and readback use the real service.
      try {
        const staged = await api("stage_core_upgrade", { id: fail ? failed : good });
        const activated = await api("activate_core_upgrade", { id: staged.stage_id });
        await route.fulfill({ json: { upgraded: activated.upgraded, from: activated.from, to: activated.to } });
      } catch (error) {
        await route.fulfill({ status: 422, json: { error: { message: (error as Error).message } } });
      }
    });
    await page.goto(`${base}/core`);
    await page.getByLabel("管理令牌").fill(token);
    await page.getByRole("button", { name: "连接服务", exact: true }).click();
    if (alpha) await page.getByLabel("升级通道").selectOption("alpha");
    const panel = page.getByRole("region", { name: `${label}内核升级` });
    const repair = panel.getByRole("button", { name: `升级至最新${label}`, exact: true });
    await expect(panel.getByText("未知（需要修复）", { exact: true })).toBeVisible();
    await expect(panel.getByText("记录未验证", { exact: true })).toBeVisible();
    await expect(repair).toBeEnabled();
    await repair.click();
    await expect(page.getByText("core activation failed; previous core restored", { exact: true })).toBeVisible();
    expect((await stat(binary)).ino).toBe(inode);
    expect((await stat(binary)).mode & 0o777).toBe(0);
    expect((await stat(binary)).size).toBe(0);
    await expect(repair).toBeEnabled();
    fail = false;
    await repair.click();
    await expect(page.getByText(`修复成功：${version}`, { exact: true })).toBeVisible();
    await expect(panel.getByText(`已验证安装 ${version}`, { exact: true })).toBeVisible();
    expect((await stat(binary)).ino).not.toBe(inode);
    expect((await api("status")).phase).toBe("stopped");
    await page.getByRole("button", { name: "退出登录", exact: true }).click();
    await stop();
    await start();
    await page.goto(`${base}/core`);
    await page.getByLabel("管理令牌").fill(token);
    await page.getByRole("button", { name: "连接服务", exact: true }).click();
    if (alpha) await page.getByLabel("升级通道").selectOption("alpha");
    await expect(panel.getByText(`已验证安装 ${version}`, { exact: true })).toBeVisible();
    await api("start");
    expect((await api("status")).phase).toBe("running");
    await page.getByRole("button", { name: "退出登录", exact: true }).click();
    expect(errors).toEqual([]);
  });
}

test("overview shows actual modes and TUN state without retaining stale state", async ({ page }) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, { method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" }, body: JSON.stringify({ command, ...fields }) });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await api("set_settings", { runtime: {} });
  const profile = await api("import_profile", { name: "Runtime overview", yaml: "mode: rule\nmixed-port: 0\ndns: {enable: false}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']" });
  await api("select_profile", { uid: profile.uid });
  await api("start");
  await page.goto(`${base}/#token=${encodeURIComponent(token)}`);
  const modes = page.getByRole("region", { name: "代理模式", exact: true });
  const tun = page.getByRole("region", { name: "TUN 模式", exact: true });
  for (const [mode, label] of [["rule", "规则"], ["global", "全局"], ["direct", "直连"]]) {
    await api("set_settings", { runtime: { mode } });
    await expect(modes.getByRole("status")).toHaveText(label);
    await expect(modes.getByRole("button", { pressed: true })).toHaveText(label);
    const access = await api("proxy_access");
    expect(access.reported.mode.toLowerCase()).toBe(mode);
    expect(access.reported.tun_enabled).toBe(false);
  }
  await expect(tun.getByRole("status")).toHaveText("已关闭");
  // This UI fixture proves that the card uses reported state, even if config disagrees.
  await page.route("**/api/commands", async route => {
    if (route.request().postDataJSON().command !== "proxy_access") return route.continue();
    const response = await route.fetch();
    const access = await response.json();
    access.reported.tun_enabled = true;
    access.tun_enabled = false;
    await route.fulfill({ json: access });
  });
  await page.getByRole("button", { name: "刷新连接信息", exact: true }).click();
  await expect(tun.getByRole("status")).toHaveText("已开启");
  await page.screenshot({ path: "test-results/overview-runtime-desktop.png", fullPage: true });
  await page.unroute("**/api/commands");
  await api("stop");
  await expect(modes.getByRole("button", { pressed: true })).toHaveText("直连");
  await expect(modes.getByText("直连 · 当前为已保存模式，内核启动后生效。")).toBeVisible();
  await expect(tun.getByRole("status")).not.toHaveText(/已开启|已关闭/);
  await api("start");
  await page.route("**/api/commands", async route => {
    if (route.request().postDataJSON().command !== "proxy_access") return route.continue();
    await route.fulfill({ status: 503, json: { error: { message: "fixture readback unavailable" } } });
  });
  await page.getByRole("button", { name: "刷新连接信息", exact: true }).click();
  await expect(modes.getByRole("status")).toHaveText("未确认");
  await expect(modes.getByRole("button", { pressed: true })).toHaveCount(0);
  for (const button of await modes.getByRole("group").getByRole("button").all()) await expect(button).toBeDisabled();
  await expect(tun.getByRole("status")).toHaveText("未确认");
  await page.unroute("**/api/commands");
});

test("settings help, stacked dismissible toasts and centered responsive content", async ({ page }) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  await page.goto(`${base}/settings#token=${encodeURIComponent(token)}`);
  const help = page.getByRole("button", { name: "运行设置帮助", exact: true });
  await expect(help).toBeVisible();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await help.hover();
  await expect(page.getByRole("tooltip")).toContainText("页面标题栏右侧的保存按钮");
  await page.mouse.move(0, 0);
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await help.focus();
  await expect(page.getByRole("tooltip")).toBeVisible();
  await help.press("Escape");
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await expect(page.locator(".settings-details[open]")).toHaveCount(0);
  const bounds = await page.locator(".shell").boundingBox();
  expect(bounds).not.toBeNull();
  expect(Math.abs(bounds!.x - (1920 - bounds!.x - bounds!.width))).toBeLessThan(2);
  const settings = await page.locator(".settings-layout").boundingBox();
  const workspace = await page.locator(".workspace").boundingBox();
  expect(Math.abs(settings!.x + settings!.width / 2 - workspace!.x - workspace!.width / 2)).toBeLessThan(2);
  const original = await settingsApi("settings");
  const initialMode = original.runtime.mode == null ? "" : String(original.runtime.mode);
  const alternateMode = initialMode === "global" ? "direct" : "global";
  await editSetting(page, "代理模式", alternateMode, true);
  await expect(page.locator(".toast")).toHaveCount(1);
  await editSetting(page, "代理模式", initialMode, true);
  await expect(page.locator(".toast")).toHaveCount(2);
  const toast = await page.locator(".toast-stack").boundingBox();
  expect(Math.abs(toast!.x + toast!.width / 2 - 960)).toBeLessThan(2);
  expect(toast!.y).toBeLessThan(30);
  await page.locator(".toast-close").first().click();
  await expect(page.locator(".toast")).toHaveCount(1);
  await page.getByRole("navigation").getByRole("link", { name: "概览", exact: true }).click();
  await expect(page.locator(".toast")).toHaveCount(1);
  await page.locator(".toast-close").click();
  await page.getByRole("navigation").getByRole("link", { name: "设置", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "混合端口", exact: true })).toBeVisible();
  await page.getByRole("textbox", { name: "混合端口", exact: true }).fill("70000");
  await applySettings(page);
  await expect(page.getByRole("alert").filter({ hasText: "0–65535" })).toBeVisible();
  await expect(page.locator(".toast")).toHaveCount(0);
  await page.getByRole("textbox", { name: "混合端口", exact: true }).fill("");
  await page.screenshot({ path: "test-results/settings-compact-desktop.png", fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await help.focus();
  const tooltip = await page.getByRole("tooltip").boundingBox();
  expect(tooltip!.x).toBeGreaterThanOrEqual(0);
  expect(tooltip!.x + tooltip!.width).toBeLessThanOrEqual(390);
  await help.press("Escape");
  await editSetting(page, "代理模式", alternateMode, true);
  await expect(page.locator(".toast")).toHaveCount(1);
  await page.screenshot({ path: "test-results/settings-compact-mobile.png", fullPage: true });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await expect(page.locator(".toast")).toHaveCount(0, { timeout: 10000 });
  await settingsApi("set_settings", { runtime: original.runtime });
});

test("core update check uses button state without a loading toast", async ({ page }) => {
  let finishCheck!: () => void;
  const heldCheck = new Promise<void>(resolve => { finishCheck = resolve; });
  await page.route("**/api/commands", async route => {
    const { command } = route.request().postDataJSON();
    if (command === "installed_core_version") return route.fulfill({ json: "v1.18.0" });
    if (command === "core_installation") return route.fulfill({ json: { version: "v1.18.0", stage_id: "fixture" } });
    if (command === "core_release") {
      await heldCheck;
      return route.fulfill({ json: { version: "v1.19.0", bytes: 1, target: "fixture" } });
    }
    return route.continue();
  });
  await page.goto(`${base}/core#token=${encodeURIComponent(token)}`);
  const panel = page.getByRole("region", { name: "稳定版内核升级" });
  const check = panel.getByRole("button", { name: "检查稳定版更新", exact: true });
  await expect(check).toBeEnabled();
  await check.click();
  await expect(check).toBeDisabled();
  await expect(page.locator(".toast")).toHaveCount(0);
  finishCheck();
  await expect(panel.locator("dd").nth(1)).toHaveText("v1.19.0");
  await expect(check).toBeEnabled();
  await expect(page.locator(".toast")).toHaveCount(0);
});

test("core upgrade reuses its operation toast for success and failure", async ({ page }) => {
  let fail = false;
  await page.route("**/api/commands", async route => {
    const { command } = route.request().postDataJSON();
    if (command === "installed_core_version") return route.fulfill({ json: "v1.18.0" });
    if (command === "core_installation") return route.fulfill({ json: { version: "v1.18.0", stage_id: "fixture" } });
    if (command === "upgrade_clash_core") return fail
      ? route.fulfill({ status: 503, json: { error: { message: "fixture upgrade failure" } } })
      : route.fulfill({ json: { upgraded: false, from: "v1.18.0", to: "v1.18.0" } });
    return route.continue();
  });
  await page.goto(`${base}/core#token=${encodeURIComponent(token)}`);
  const upgrade = page.getByRole("button", { name: "升级至最新稳定版", exact: true });
  await expect(upgrade).toBeEnabled();
  await upgrade.click();
  await expect(page.locator(".toast")).toHaveCount(1);
  await expect(page.locator(".toast")).toContainText("已是最新稳定版 v1.18.0");
  await page.locator(".toast-close").click();
  fail = true;
  await expect(upgrade).toBeEnabled();
  await upgrade.click();
  await expect(page.getByRole("alert").filter({ hasText: "fixture upgrade failure" })).toBeVisible();
  await expect(page.locator(".toast-error")).toHaveCount(1);
  await expect(page.locator(".toast-error")).toContainText("fixture upgrade failure");
});

for (const outcome of ["success", "error"] as const) {
  test(`operation toast stays visible without shifting content and becomes ${outcome}`, async ({ page }) => {
    const initial = await fetch(`${base}/api/commands`, {
      method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify({ command: "edit_config", yaml: "mode: direct\nmixed-port: 0\ndns: {enable: false}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']" }),
    });
    expect(initial.ok).toBe(true);
    await page.goto(`${base}/config#token=${encodeURIComponent(token)}`);
    const input = page.getByRole("textbox", { name: "运行配置 YAML", exact: true });
    await expect(input).toBeEnabled();
    await input.fill("mode: direct\nmixed-port: 0\ndns: {enable: false}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']");
    const panel = page.locator(".workspace > .panel");
    const before = await panel.boundingBox();
    let release!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    let releaseReadback!: () => void;
    const readbackGate = new Promise<void>(resolve => { releaseReadback = resolve; });
    let submitted = false, readingBack = false;
    await page.route("**/api/commands", async route => {
      const { command } = route.request().postDataJSON();
      if (command === "edit_config") {
        submitted = true;
        await gate;
        if (outcome === "error") return route.fulfill({ status: 503, json: { error: { message: "fixture save failure" } } });
      } else if (submitted && command === "status") {
        readingBack = true;
        await readbackGate;
      }
      await route.continue();
    });
    await page.clock.install({ time: new Date("2026-10-04T00:00:00Z") });
    await page.clock.pauseAt(new Date("2026-10-04T00:00:01Z"));
    try {
      await page.getByRole("button", { name: "校验并应用", exact: true }).click();
      const toast = page.locator(".toast");
      await expect(toast).toHaveCount(1);
      await expect(toast).toHaveClass("toast toast-loading");
      const id = await toast.getAttribute("data-toast-id");
      const loadingColor = await toast.evaluate(node => getComputedStyle(node).backgroundColor);
      await expect(toast.locator(".toast-spinner")).toBeVisible();
      expect(await toast.locator(".toast-spinner").evaluate(node => getComputedStyle(node).animationName)).toBe("toast-spin");
      await expect(toast.locator(".toast-close")).toHaveCount(0);
      expect((await panel.boundingBox())!.y).toBe(before!.y);
      await expect(page.locator(".feedback")).not.toContainText("正在处理");
      await page.clock.runFor(9000);
      await expect(toast).toHaveClass("toast toast-loading");
      release();
      if (outcome === "success") {
        await expect.poll(() => readingBack).toBe(true);
        await page.clock.runFor(9000);
        await expect(toast).toHaveClass("toast toast-loading");
      }
      releaseReadback();
      await expect(toast).toHaveClass(`toast toast-${outcome}`);
      await expect(toast).toHaveAttribute("data-toast-id", id!);
      await expect(toast).toContainText(outcome === "success" ? "保存成功" : "fixture save failure");
      await expect(toast.locator(".toast-spinner")).toHaveCount(0);
      expect(await toast.evaluate(node => getComputedStyle(node).backgroundColor)).not.toBe(loadingColor);
      expect((await panel.boundingBox())!.y).toBe(before!.y);
      await expect(toast.locator(".toast-close")).toBeVisible();
      await page.setViewportSize({ width: 390, height: 844 });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
      await page.screenshot({ path: `test-results/operation-toast-${outcome}-mobile.png`, fullPage: true });
      await page.clock.runFor(7999);
      await expect(toast).toHaveCount(1);
      await page.clock.runFor(1);
      await expect(toast).toHaveCount(0);
    } finally {
      release();
      releaseReadback();
    }
  });
}

test("repeated configuration saves produce separate success toasts", async ({ page }) => {
  await page.goto(`${base}/config#token=${encodeURIComponent(token)}`);
  const input = page.getByRole("textbox", { name: "运行配置 YAML", exact: true });
  await expect(input).toBeEnabled();
  await input.fill("mode: direct\nmixed-port: 0\ndns: {enable: false}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']");
  const save = page.getByRole("button", { name: "校验并应用", exact: true });
  await save.click();
  await expect(page.locator(".toast")).toHaveCount(1);
  await expect(save).toBeEnabled();
  await save.click();
  await expect(page.locator(".toast")).toHaveCount(2);
  await expect(page.locator(".toast").first()).toContainText("保存成功");
  await expect(page.locator(".toast").last()).toContainText("保存成功");
});

test("overview TUN switch and compact settings row preserve advanced settings and auto-apply", async ({ page }) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, { method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" }, body: JSON.stringify({ command, ...fields }) });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await api("stop");
  await api("set_settings", { runtime: {} });
  const profile = await api("import_profile", { name: "TUN controls", yaml: "mode: direct\nmixed-port: 0\ndns: {enable: false, nameserver: [1.1.1.1]}\ntun: {enable: false, auto-route: false}\nrules: ['MATCH,DIRECT']" });
  await api("select_profile", { uid: profile.uid });
  const runtime = { mode: "direct", "mixed-port": 0, ipv6: false, tun: { enable: false, stack: "mixed", mtu: 1400, "auto-route": false, "dns-hijack": [] } };
  const before = await api("set_settings", { runtime });
  await page.goto(`${base}/#token=${encodeURIComponent(token)}`);
  const toggle = page.getByRole("switch", { name: "TUN 模式", exact: true });
  await expect(toggle).toBeEnabled();
  await expect(toggle).toHaveAttribute("aria-checked", "false");
  await page.screenshot({ path: "test-results/tun-switch-overview-desktop.png", fullPage: true });
  await toggle.click();
  await expect(page.locator(".toast").filter({ hasText: "TUN：已开启" })).toBeVisible();
  await expect(toggle).toBeEnabled();
  await expect(toggle).toHaveAttribute("aria-checked", "true");
  expect(await api("settings")).toEqual({ ...before, runtime: { ...runtime, tun: { ...runtime.tun, enable: true } } });
  expect((await api("status")).phase).toBe("stopped");
  await toggle.focus(); await toggle.press("Space");
  await expect(page.locator(".toast").filter({ hasText: "TUN：已关闭" })).toBeVisible();
  await expect(toggle).toBeEnabled();
  await expect(toggle).toHaveAttribute("aria-checked", "false");
  expect(await api("settings")).toEqual(before);
  await api("start");
  let writes = 0;
  // Permission failures leave the switch at the actual value and keep the running core.
  await page.route("**/api/commands", async route => {
    if (route.request().postDataJSON().command !== "set_tun_enabled") return route.continue();
    writes++;
    await route.fulfill({ status: 403, json: { error: { message: "fixture TUN permission denied" } } });
  });
  await expect(toggle).toBeEnabled();
  const running = await api("status");
  await toggle.click();
  await expect(page.getByRole("alert").filter({ hasText: "fixture TUN permission denied" })).toBeVisible();
  await expect(toggle).toBeEnabled();
  await expect(toggle).toHaveAttribute("aria-checked", "false");
  expect(writes).toBe(1);
  expect((await api("status")).pid).toBe(running.pid);
  expect(await api("settings")).toEqual(before);
  await page.unroute("**/api/commands");
  // Project a failed startup snapshot while the actual test core stays stopped.
  await api("stop");
  await page.routeWebSocket("**/api/events", socket => {
    const server = socket.connectToServer();
    server.onMessage(message => {
      if (typeof message === "string") {
        const event = JSON.parse(message);
        if (event.type === "snapshot") event.status.phase = "failed";
        socket.send(JSON.stringify(event));
      } else socket.send(message);
    });
  });
  await page.goto(`${base}/?token=${encodeURIComponent(token)}`);
  await expect(page.getByRole("region", { name: "TUN 模式", exact: true }).getByRole("status")).toHaveText("启动失败");
  await expect(toggle).toBeEnabled();
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "true");
  await expect(toggle).toBeEnabled();
  expect((await api("status")).phase).toBe("stopped");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "false");
  await expect(toggle).toBeEnabled();
  expect(await api("settings")).toEqual(before);
  await page.getByRole("navigation").getByRole("link", { name: "设置", exact: true }).click();
  const tun = page.getByRole("combobox", { name: "TUN 模式", exact: true });
  await expect(tun).toBeEnabled();
  await expect(page.locator(".settings-group[open]")).toHaveCount(0);
  await expect(tun).toBeInViewport();
  const mode = page.getByRole("combobox", { name: "代理模式", exact: true });
  await mode.focus(); await mode.selectOption("global");
  await expect(tun).toBeEnabled();
  await expect(page.getByRole("button", { name: "保存服务设置", exact: true })).toBeEnabled();
  await expect(page.getByText("请先应用或修正尚未生效的设置，再更改 TUN。")).toHaveCount(0);
  await mode.selectOption("direct");
  await expect(page.getByRole("button", { name: "保存服务设置", exact: true })).toBeDisabled();
  await expect(tun).toBeEnabled();
  await tun.focus(); await tun.selectOption("true");
  expect(await api("settings")).toEqual(before);
  await applySettings(page);
  await expect(tun).toBeEnabled();
  expect(await api("settings")).toEqual({ ...before, runtime: { ...runtime, tun: { ...runtime.tun, enable: true } } });
  await expect(page.getByRole("button", { name: "保存服务设置", exact: true })).toBeDisabled();
  await expect(page.getByText("有待应用的修改", { exact: true })).toHaveCount(0);
  const row = page.locator(".tun-setting-row");
  const bounds = await row.boundingBox();
  expect(bounds!.height).toBeLessThanOrEqual(50);
  await expect(row.locator(".tun-indicator, .help-tip")).toHaveCount(0);
  const label = await row.locator("label").boundingBox(), control = await tun.boundingBox();
  expect(Math.abs(label!.y + label!.height / 2 - control!.y - control!.height / 2)).toBeLessThan(2);
  while (await page.locator(".toast-close").count()) await page.locator(".toast-close").first().click();
  await page.screenshot({ path: "test-results/tun-switch-settings-desktop.png", fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: "test-results/tun-switch-settings-mobile.png", fullPage: true });
  await expect(tun).toBeInViewport();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await tun.focus(); await tun.selectOption("false"); await applySettings(page);
  await expect(tun).toBeEnabled();
  expect(await api("settings")).toEqual(before);
});

test("overview mode button group applies immediately, verifies live state and preserves other settings", async ({ page }) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, { method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" }, body: JSON.stringify({ command, ...fields }) });
    expect(response.ok).toBe(true);
    return response.json();
  };
  await api("stop");
  await api("set_settings", { runtime: {} });
  const profile = await api("import_profile", { name: "Mode button group", yaml: "mode: rule\nmixed-port: 0\ndns: {enable: false, nameserver: [1.1.1.1]}\ntun: {enable: false, auto-route: false}\nrules: ['MATCH,DIRECT']" });
  await api("select_profile", { uid: profile.uid });
  const runtime = { mode: "rule", "mixed-port": 0, ipv6: false, dns: { nameserver: ["1.1.1.1"] }, tun: { enable: false, stack: "mixed", mtu: 1400, "auto-route": false, "dns-hijack": [] } };
  const before = await api("set_settings", { runtime });
  await api("start");
  let writes = 0;
  page.on("request", request => { if (request.url().endsWith("/api/commands") && request.postDataJSON()?.command === "set_proxy_mode") writes++; });
  await page.goto(`${base}/#token=${encodeURIComponent(token)}`);
  const card = page.getByRole("region", { name: "代理模式", exact: true });
  const group = card.getByRole("group", { name: "代理模式", exact: true });
  await expect(group.getByRole("button")).toHaveCount(3);
  await expect(group.getByRole("button", { pressed: true })).toHaveText("规则");
  for (const [mode, label] of [["direct", "直连"], ["global", "全局"], ["rule", "规则"]]) {
    const button = group.getByRole("button", { name: label, exact: true });
    await expect(button).toBeEnabled();
    if (mode === "direct") { await button.focus(); await button.press("Enter"); } else await button.click();
    await expect(card.getByRole("status")).toHaveText(label);
    await expect(group.getByRole("button", { pressed: true })).toHaveText(label);
    await expect(button).toBeEnabled();
    await expect(page.locator(".toast").filter({ hasText: `代理模式：${label}` })).toBeVisible();
    expect(await api("settings")).toEqual({ ...before, runtime: { ...runtime, mode } });
    expect((await api("proxy_access")).reported.mode.toLowerCase()).toBe(mode);
  }
  expect(writes).toBe(3);
  await group.getByRole("button", { name: "规则", exact: true }).click();
  expect(writes).toBe(3);
  const failedMode = async (route: import("@playwright/test").Route) => {
    if (route.request().postDataJSON().command !== "set_proxy_mode") return route.continue();
    await route.fulfill({ status: 422, json: { error: { message: "fixture mode apply rejected" } } });
  };
  await page.route("**/api/commands", failedMode);
  await group.getByRole("button", { name: "全局", exact: true }).click();
  await expect(page.getByRole("alert").filter({ hasText: "fixture mode apply rejected" })).toBeVisible();
  await expect(group.getByRole("button", { name: "全局", exact: true })).toBeEnabled();
  await expect(group.getByRole("button", { pressed: true })).toHaveText("规则");
  expect(await api("settings")).toEqual(before);
  await page.unroute("**/api/commands", failedMode);
  const lostMode = async (route: import("@playwright/test").Route) => {
    if (route.request().postDataJSON().command !== "set_proxy_mode") return route.continue();
    await route.fetch(); await route.abort("failed");
  };
  await page.route("**/api/commands", lostMode);
  await group.getByRole("button", { name: "全局", exact: true }).click();
  await expect(page.locator(".toast-info").filter({ hasText: "代理模式：全局" })).toBeVisible();
  await expect(group.getByRole("button", { pressed: true })).toHaveText("全局");
  await expect(group.getByRole("button", { name: "规则", exact: true })).toBeEnabled();
  expect((await api("settings")).runtime).toEqual({ ...runtime, mode: "global" });
  await page.unroute("**/api/commands", lostMode);
  await api("stop");
  await expect(card.getByRole("status")).toHaveText("已停止");
  await expect(group.getByRole("button", { name: "规则", exact: true })).toBeEnabled();
  await group.getByRole("button", { name: "规则", exact: true }).click();
  await expect(group.getByRole("button", { pressed: true })).toHaveText("规则");
  await expect(group.getByRole("button", { name: "规则", exact: true })).toBeEnabled();
  expect((await api("status")).phase).toBe("stopped");
  expect(await api("settings")).toEqual(before);
  await api("start");
  await expect(card.getByRole("status")).toHaveText("规则");
  while (await page.locator(".toast-close").count()) await page.locator(".toast-close").first().click();
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({ path: "test-results/mode-button-group-desktop.png" });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({ path: "test-results/mode-button-group-mobile.png" });
  await expect(group).toBeInViewport();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await group.getByRole("button", { name: "直连", exact: true }).click();
  await expect(card.getByRole("status")).toHaveText("直连");
  expect((await api("proxy_access")).reported.mode.toLowerCase()).toBe("direct");
});

test("settings read failures and unsupported schemas cannot publish empty replacements", async ({ page }) => {
  let response: "failure" | "unknown" | "normal" = "failure", writes = 0;
  await page.route("**/api/commands", async route => {
    const { command } = route.request().postDataJSON();
    if (command === "set_settings") writes++;
    if (command === "settings" && response === "failure") return route.fulfill({ status: 503, json: { error: { message: "fixture initial read failed" } } });
    if (command === "settings" && response === "unknown") return route.fulfill({ json: { schema_version: 1, runtime: { "future-setting": true } } });
    await route.continue();
  });
  await page.goto(`${base}/settings#token=${encodeURIComponent(token)}`);
  await expect(page.getByRole("alert")).toContainText("fixture initial read failed");
  await expect(page.getByRole("form", { name: "运行设置表单" })).toHaveCount(0);
  response = "unknown";
  await page.getByRole("button", { name: "重试读取设置", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("暂不支持的设置 future-setting");
  await expect(page.getByRole("form", { name: "运行设置表单" })).toHaveCount(0);
  expect(writes).toBe(0);
  response = "normal";
  await page.getByRole("button", { name: "重试读取设置", exact: true }).click();
  await expect(page.getByRole("form", { name: "运行设置表单" })).toBeVisible();
  expect(writes).toBe(0);
});

test("settings language applies on change and is shared through the service", async ({ page }) => {
  await loginSettings(page);
  const before = await settingsApi("settings");
  try {
    const language = page.getByRole("combobox", { name: "界面语言", exact: true });
    await language.selectOption("en");
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await expect.poll(async () => (await settingsApi("preferences")).language).toBe("en");
    expect(await settingsApi("settings")).toEqual(before);
    // A change made by another client (the desktop client, another window) arrives as an event.
    await settingsApi("set_language", { language: "zhtw" });
    await expect(page.locator("html")).toHaveAttribute("lang", "zh-TW");
    await expect(page.getByRole("combobox", { name: "介面語言", exact: true })).toHaveValue("zhtw");
    // A browser without a local choice adopts the instance's language once logged in.
    const other = await page.context().browser()!.newPage();
    await other.goto(`${base}/#token=${encodeURIComponent(token)}`);
    await expect(other.locator("html")).toHaveAttribute("lang", "zh-TW");
    await other.close();
  } finally {
    await settingsApi("set_language", { language: null });
  }
});

test("settings uses the compact layout and shows browser-safe version information", async ({ page, browser }) => {
  await loginSettings(page);
  await expect(page.getByText("工作空间", { exact: true })).toHaveCount(0);
  await expect(page.getByText("独立运行 · 配置与节点选择由服务保存", { exact: true })).toHaveCount(0);
  for (const removed of ["内核实际设置", "当前订阅 DNS 覆盖", "已保存服务设置"])
    await expect(page.getByText(removed, { exact: true })).toHaveCount(0);

  const versions = page.getByRole("region", { name: "版本信息" });
  await expect(versions).toBeVisible();
  await expect(versions.getByText("mihomo内核版本", { exact: true })).toBeVisible();
  const serviceVersion = await settingsApi("service_version");
  await expect(versions.locator("div").filter({ hasText: "服务端版本" }).locator("dd")).toHaveText(serviceVersion);
  await expect(versions.getByText("桌面客户端版本", { exact: true })).toHaveCount(0);
  const selectHeight = await page.getByRole("combobox", { name: "代理模式", exact: true }).evaluate(element => element.getBoundingClientRect().height);
  expect(selectHeight).toBeLessThanOrEqual(32);

  const desktop = await browser.newContext();
  await desktop.addInitScript(() => {
    Object.defineProperty(window, "__MIHOMO_DESKTOP_VERSION__", { value: "0.1.0-desktop-test" });
  });
  try {
    const desktopPage = await desktop.newPage();
    await desktopPage.goto(`${base}/settings#token=${encodeURIComponent(token)}`);
    const desktopVersions = desktopPage.getByRole("region", { name: "版本信息" });
    await expect(desktopVersions.locator("div").filter({ hasText: "桌面客户端版本" }).locator("dd")).toHaveText("0.1.0-desktop-test");
  } finally {
    await desktop.close();
  }
});

test("service page controls the foreground service and keeps long help in tooltips", async ({ page }) => {
  await page.route("**/api/commands", async route => {
    if (route.request().postDataJSON()?.command === "service_release") await route.fulfill({ json: "v9.9.9" });
    else await route.continue();
  });
  await page.goto(`${base}/service#token=${encodeURIComponent(token)}`);
  const navigation = page.getByRole("navigation", { name: "主导航" });
  await expect(navigation.getByRole("link", { name: "服务", exact: true })).toHaveAttribute("aria-current", "page");
  expect((await navigation.getByRole("link").allTextContents()).slice(-2)).toEqual(["内核", "服务"]);
  await expect(page.getByRole("heading", { level: 1, name: "服务" })).toBeVisible();

  const control = page.getByRole("region", { name: "服务控制" });
  await expect(control.locator("div").filter({ hasText: "服务版本" }).locator("dd")).toHaveText(await settingsApi("service_version"));
  await expect(control.getByText("直接启动", { exact: true })).toBeVisible();
  await expect(control.getByRole("button", { name: "重启服务" })).toBeDisabled();
  await expect(control.getByRole("button", { name: "停止服务" })).toBeEnabled();
  await expectHelp(page, "服务控制", "mihomo-server 是托管 Mihomo 内核并提供本管理页面的后台服务。");
  await expectHelp(page, "直接启动", "服务不是由 systemd 启动的，只能停止，请在启动它的地方重新启动。");

  const upgrade = page.getByRole("region", { name: "服务升级" });
  await expect(upgrade.getByText("非共享安装", { exact: true })).toBeVisible();
  await expect(upgrade.getByText("不可用", { exact: true })).toBeVisible();
  await expect(upgrade.getByLabel("升级输出")).toBeHidden();
  await expect(upgrade.getByLabel("升级输出")).toHaveText("需要共享安装才能在此升级。");
  await expect(upgrade.getByRole("button", { name: "升级至最新版" })).toBeDisabled();
  await expectHelp(page, "服务升级", "升级由系统更新单元以管理员权限完成");
  await expectHelp(page, "不可用", "请在终端运行 mihomo-server update 升级一次。");
  await upgrade.getByRole("button", { name: "检查更新" }).click();
  await expect(upgrade.locator("div").filter({ hasText: "最新版本" }).locator("dd")).toHaveText("v9.9.9");

  // The management address logs a browser straight in through the fragment.
  const address = page.getByRole("link", { name: `${base}/#token=${encodeURIComponent(token)}`, exact: true });
  await expect(page.getByText("网页管理地址：")).toBeVisible();
  await expect(address).toHaveAttribute("href", `${base}/#token=${encodeURIComponent(token)}`);
  await expect(address).toHaveAttribute("target", "_blank");
  await expectHelp(page, "网页管理地址：", "链接含登录令牌，请妥善保管，勿分享给他人。");
  const [opened] = await Promise.all([page.context().waitForEvent("page"), address.click()]);
  await expect(opened.getByRole("heading", { name: "概览", exact: true })).toBeVisible();
  expect(opened.url()).not.toContain("token");
  await opened.close();

  // A dismissed confirmation stops nothing; reloading keeps the page.
  page.once("dialog", dialog => dialog.dismiss());
  await control.getByRole("button", { name: "停止服务" }).click();
  await page.reload();
  await expect(control.getByRole("button", { name: "停止服务" })).toBeEnabled();
  expect((await settingsApi("service_info")).unit).toBeNull();

  for (const [command, message] of [["restart_service", "started directly"], ["upgrade_service", "shared installation"]]) {
    const response = await fetch(`${base}/api/commands`, {
      method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify({ command }),
    });
    expect(response.ok).toBe(false);
    expect(JSON.stringify(await response.json())).toContain(message);
  }
  await page.unroute("**/api/commands");
});

test("service restart and upgrade report progress without moving the page", async ({ page }) => {
  const sockets: import("@playwright/test").WebSocketRoute[] = [];
  await page.routeWebSocket("**/api/events", socket => {
    sockets.push(socket);
    socket.onMessage(message => {
      if (JSON.parse(String(message)).type === "authenticate") {
        socket.send(JSON.stringify({ type: "ready" }));
        socket.send(JSON.stringify({ type: "status", data: { phase: "running", generation: 0, selection_pending: [] } }));
      }
    });
  });
  const disconnect = () => sockets.splice(0).forEach(socket => void socket.close());
  let info = { version: "0.2.0", release: "v0.2.0", unit: "mihomo-server.service", upgrade: { available: true, state: "inactive", result: "success", log: [] as string[] } };
  let failNext = false;
  await page.route("**/api/commands", async route => {
    const command = route.request().postDataJSON()?.command;
    if (command === "service_info") await route.fulfill({ json: info });
    else if (command === "service_release") await route.fulfill({ json: "v0.3.0" });
    else if (command === "restart_service") {
      await route.fulfill({ json: { action: "restart" } });
      setTimeout(disconnect, 100);
    } else if (command === "upgrade_service") {
      info = { ...info, upgrade: { ...info.upgrade, state: "activating", log: ["==> downloading mihomo-server-v0.3.0"] } };
      if (failNext) setTimeout(() => { info = { ...info, upgrade: { ...info.upgrade, state: "failed", result: "exit-code", log: ["error: checksum mismatch"] } }; }, 2500);
      await route.fulfill({ json: { unit: "mihomo-server-update.service" } });
    } else await route.continue();
  });
  await page.goto(`${base}/service#token=${encodeURIComponent(token)}`);
  const control = page.getByRole("region", { name: "服务控制" });
  const upgrade = page.getByRole("region", { name: "服务升级" });
  const output = upgrade.getByLabel("升级输出");
  await expect(control.getByText("mihomo-server.service", { exact: true })).toBeVisible();
  await expect(output).toHaveText("暂无升级输出");
  // Collapsed by default, and the buttons keep a clear gap below the output.
  await expect(output).toBeHidden();
  await upgrade.getByText("升级输出", { exact: true }).click();
  await expect(output).toBeVisible();
  const outputBox = (await output.boundingBox())!, buttonsBox = (await upgrade.locator(".actions").boundingBox())!;
  expect(buttonsBox.y - (outputBox.y + outputBox.height)).toBeGreaterThanOrEqual(16);

  page.once("dialog", dialog => dialog.accept());
  await control.getByRole("button", { name: "重启服务" }).click();
  await expect(page.locator(".toast").filter({ hasText: "服务已重启" })).toBeVisible();

  await upgrade.getByRole("button", { name: "检查更新" }).click();
  await expect(upgrade.locator("div").filter({ hasText: "最新版本" }).locator("dd")).toHaveText("v0.3.0");
  const actions = await upgrade.locator(".actions").boundingBox();
  page.once("dialog", dialog => dialog.accept());
  await upgrade.getByRole("button", { name: "升级至最新版" }).click();
  await expect(upgrade.getByText("升级中…", { exact: true })).toBeVisible();
  await expect(output).toContainText("downloading mihomo-server-v0.3.0");
  info = { ...info, upgrade: { ...info.upgrade, log: Array.from({ length: 40 }, (_, index) => `line ${index}`) } };
  await expect(output).toContainText("line 39");
  expect(await upgrade.locator(".actions").boundingBox()).toEqual(actions);
  expect(await output.evaluate(element => element.scrollTop + element.clientHeight >= element.scrollHeight - 1)).toBe(true);

  // The installer restarts the service, which comes back upgraded.
  info = { ...info, version: "0.3.0", release: "v0.3.0", upgrade: { ...info.upgrade, state: "inactive", log: [...info.upgrade.log, "System install complete"] } };
  disconnect();
  await expect(page.locator(".toast").filter({ hasText: "已升级至 v0.3.0" })).toBeVisible();
  await expect(upgrade.getByRole("button", { name: "已是最新版本 v0.3.0" })).toBeDisabled();
  await expect(control.locator("div").filter({ hasText: "服务版本" }).locator("dd")).toHaveText("0.3.0");

  // A failed run is reported with its output.
  info = { ...info, version: "0.2.0", release: "v0.2.0" };
  failNext = true;
  await page.reload();
  await upgrade.getByRole("button", { name: "检查更新" }).click();
  page.once("dialog", dialog => dialog.accept());
  await upgrade.getByRole("button", { name: "升级至最新版" }).click();
  await expect(page.locator(".toast").filter({ hasText: "升级失败，详见升级输出" })).toBeVisible();
  await expect(upgrade.getByText("上次升级失败", { exact: true })).toBeVisible();
  await expect(output).toHaveText("error: checksum mismatch");
  await page.unroute("**/api/commands");
});

test("settings start at login separates the service unit from the desktop client", async ({ page, browser }) => {
  await loginSettings(page);
  const panel = page.getByRole("region", { name: "登录时启动" });
  const service = panel.getByRole("switch", { name: "服务端登录时启动" });
  // A foreground service has no unit to enable; browsers never see the client switch.
  await expect(service).toBeDisabled();
  await expect(service).toHaveAttribute("aria-checked", "false");
  await expectHelp(page, "服务端登录时启动", "当前服务是手动启动的（例如在终端中直接运行）");
  await expectHelp(page, "登录时启动", "客户端默认不随登录启动");
  await expect(panel.getByRole("switch", { name: "客户端登录时启动" })).toHaveCount(0);
  const response = await fetch(`${base}/api/commands`, {
    method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    body: JSON.stringify({ command: "set_service_autostart", enabled: true }),
  });
  expect(response.ok).toBe(false);
  expect(JSON.stringify(await response.json())).toContain("started directly");

  const desktop = await browser.newContext();
  await desktop.addInitScript(() => {
    const calls: unknown[] = [];
    let client = false;
    Object.defineProperty(window, "__MIHOMO_DESKTOP_VERSION__", { value: "0.1.0-desktop-test" });
    Object.defineProperty(window, "__desktopCalls", { value: calls });
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      value: {
        invoke: async (command: string, args?: { enabled: boolean }) => {
          calls.push([command, args ?? null]);
          if (command === "set_client_autostart") client = args!.enabled;
          return client;
        },
      },
    });
  });
  try {
    const desktopPage = await desktop.newPage();
    let serviceAutostart = true;
    await desktopPage.route("**/api/commands", async route => {
      const body = route.request().postDataJSON();
      if (body?.command === "service_info")
        await route.fulfill({ json: { version: "0.2.0", release: "v0.2.0", unit: "mihomo-server.service", autostart: serviceAutostart, upgrade: { available: true, state: "inactive", result: "success", log: [] } } });
      else if (body?.command === "set_service_autostart") {
        serviceAutostart = body.enabled;
        await route.fulfill({ json: serviceAutostart });
      } else await route.continue();
    });
    await desktopPage.goto(`${base}/settings#token=${encodeURIComponent(token)}`);
    const desktopPanel = desktopPage.getByRole("region", { name: "登录时启动" });
    const serviceSwitch = desktopPanel.getByRole("switch", { name: "服务端登录时启动" });
    const clientSwitch = desktopPanel.getByRole("switch", { name: "客户端登录时启动" });
    await expect(serviceSwitch).toHaveAttribute("aria-checked", "true");
    await expect(clientSwitch).toHaveAttribute("aria-checked", "false");
    await serviceSwitch.click();
    await expect(serviceSwitch).toHaveAttribute("aria-checked", "false");
    expect(serviceAutostart).toBe(false);
    await clientSwitch.click();
    await expect(clientSwitch).toHaveAttribute("aria-checked", "true");
    expect(await desktopPage.evaluate(() => (window as unknown as { __desktopCalls: unknown[] }).__desktopCalls)).toEqual([
      ["client_autostart", null],
      ["set_client_autostart", { enabled: true }],
    ]);
  } finally {
    await desktop.close();
  }
});
