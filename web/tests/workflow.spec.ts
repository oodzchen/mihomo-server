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
      : resolve("../target/debug/mihomo-server"),
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
          resolve("dist"),
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
  const listener = createServer();
  await new Promise<void>((resolve) =>
    listener.listen(0, "127.0.0.1", resolve),
  );
  const address = listener.address();
  if (!address || typeof address === "string")
    throw new Error("No test address");
  base = `http://127.0.0.1:${address.port}`;
  await new Promise<void>((resolve) => listener.close(() => resolve()));
  await start();
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
  await expect(navigation.getByRole("link", { name: "01 Overview" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Start core", exact: true })).toBeVisible();
  const before = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.getByRole("navigation", { name: "主导航" }).getByRole("link", { name: "01 概览" })).toBeVisible();
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
  await expect(page.getByRole("heading", { name: "Import local profile" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Download remote profile" })).toBeVisible();
  await page.getByLabel("Upload profile YAML").setInputFiles({
    name: "large.yaml", mimeType: "text/yaml", buffer: Buffer.alloc(8 * 1024 ** 2 + 1),
  });
  await expect(page.getByText("File must not exceed 8 MiB")).toBeVisible();
  await page.getByLabel("Subscription URL").fill(`${subscriptionUrl}/ok`);
  await page.getByLabel("Remote profile name (optional)").fill("RemoteDraft");
  await page.getByLabel("Allow invalid TLS certificates for downloads").check();
  const before = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());

  await page.getByRole("combobox", { name: "Interface language" }).selectOption("zh");
  await expect(page.getByText("文件不能超过 8 MiB")).toBeVisible();
  await expect(page.getByLabel("订阅链接")).toHaveValue(`${subscriptionUrl}/ok`);
  await expect(page.getByLabel("远程订阅名称（可选）")).toHaveValue("RemoteDraft");
  await expect(page.getByLabel("下载允许无效 TLS 证书")).toBeChecked();
  await page.getByLabel("上传订阅 YAML").setInputFiles({
    name: "LocalDraft.yaml", mimeType: "text/yaml", buffer: Buffer.from("proxies: []\nmode: direct\n"),
  });
  await expect(page.getByLabel("订阅名称", { exact: true })).toHaveValue("LocalDraft");
  await expect(page.getByText("文件不能超过 8 MiB")).toHaveCount(0);
  const after = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
  expect(after.generation).toBe(before.generation);

  await page.getByRole("combobox", { name: "界面语言" }).selectOption("en");
  await page.getByRole("button", { name: "Import profile", exact: true }).click();
  await expect(page.getByRole("heading", { name: "LocalDraft" })).toBeVisible();
  await page.getByLabel("Allow invalid TLS certificates for downloads").uncheck();
  await page.getByRole("button", { name: "Download and import" }).click();
  await expect(page.getByRole("heading", { name: "RemoteDraft" })).toBeVisible();
  for (const profile of ["LocalDraft", "RemoteDraft"]) {
    const item = page.locator("article.profile").filter({ has: page.getByRole("heading", { name: profile }) });
    await item.getByRole("button", { name: `Delete profile ${profile}` }).click();
    await item.getByRole("button", { name: `Confirm deletion ${profile}` }).click();
  }
  await expect(page.getByText("No profiles yet. Import a YAML file to get started.")).toBeVisible();
});

test("profile metadata editor keeps remote draft across language changes and saves it", async ({ page }) => {
  await page.goto(`${base}/profiles`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await page.getByLabel("订阅链接").fill(`${subscriptionUrl}/ok`);
  await page.getByLabel("远程订阅名称（可选）").fill("MetadataDraft");
  await page.getByRole("button", { name: "下载并导入" }).click();
  const draft = page.locator("article.profile").filter({ has: page.getByRole("heading", { name: "MetadataDraft" }) });
  await expect(draft).toBeVisible();
  const uid = await draft.locator(".mono").textContent();
  const before = await fetch(`${base}/api/status`, { headers: { Authorization: `Bearer ${token}` } }).then(response => response.json());
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
  await saved.getByRole("button", { name: "Delete profile MetadataSaved" }).click();
  await saved.getByRole("button", { name: "Confirm deletion MetadataSaved" }).click();
  await expect(page.getByText("No profiles yet. Import a YAML file to get started.")).toBeVisible();
});

test("resource inventory reads metadata, refreshes changes and retries without editing settings", async ({ page }) => {
  await page.goto(`${base}/settings`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
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
    await expect(panel).toContainText("尚未验证内容格式");
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
  await expect(panel.getByRole("status")).toContainText("已安装打包资源");
  await expect(panel.getByRole("status")).toContainText("完整结构未验证");
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
  await expect(panel.getByRole("status")).toContainText("DAT 结构及隔离内核规则加载通过");
  await expect(panel.getByRole("status")).toContainText(seedHash);
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
  await update.click(); await expect(panel.getByRole("status")).toContainText("已安装在线资源");
  await expect(panel.getByRole("status")).toContainText(newHash);
  expect(reads).toBe(2); expect(updates).toBe(2);
  await page.unroute("**/api/commands"); await page.getByRole("button", { name: "退出登录" }).click();
});

test("connection settings save explicit false, preserve failed drafts and retry actual readback", async ({ page }) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, { method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" }, body: JSON.stringify({ command, ...fields }) });
    expect(response.ok).toBe(true); return response.json();
  };
  const original = await api("settings");
  await page.goto(`${base}/settings`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  const form = page.getByRole("region", { name: "服务设置编辑器", exact: true });
  const panel = page.getByRole("region", { name: "连接设置读回", exact: true });
  const tcp = page.getByRole("combobox", { name: "TCP 并发连接", exact: true });
  const mode = page.getByRole("combobox", { name: "进程匹配模式", exact: true });
  const interval = page.getByRole("textbox", { name: "TCP 保活间隔（秒）", exact: true });
  const idle = page.getByRole("textbox", { name: "TCP 保活空闲时间（秒）", exact: true });
  const disable = page.getByRole("combobox", { name: "禁用 TCP 保活", exact: true });
  const managedInterface = page.getByRole("checkbox", { name: "管理出口网卡", exact: true });
  const interfaceName = page.getByRole("textbox", { name: "出口网卡名称", exact: true });
  const managedAgent = page.getByRole("checkbox", { name: "管理核心下载 User-Agent", exact: true });
  const agent = page.getByRole("textbox", { name: "核心下载 User-Agent", exact: true });
  const etag = page.getByRole("combobox", { name: "核心下载 ETag", exact: true });
  const agentRow = panel.locator("li").filter({ has: page.getByText("global-ua", { exact: true }) });
  const mark = page.getByRole("textbox", { name: "Linux 路由标记", exact: true });
  const interfaceRow = panel.locator("li").filter({ has: page.getByText("interface-name", { exact: true }) });
  const row = panel.locator("li").filter({ has: page.getByText("tcp-concurrent", { exact: true }) });
  try {
    await expect(panel).toContainText("尚无已提交配置");
    await expect(managedInterface).not.toBeChecked(); await expect(interfaceName).toBeDisabled();
    for (const invalid of ["2147483648", "-2147483649", "1.5", "abc"]) {
      await interval.fill(invalid);
      await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
      await expect(form.getByRole("alert")).toContainText("必须是 -2147483648–2147483647");
      expect(await api("settings")).toEqual(original);
    }
    await interval.fill("0"); await idle.fill("-1"); await disable.selectOption("false");
    await tcp.selectOption("false"); await mode.selectOption("off");
    await page.getByRole("combobox", { name: "IPv6", exact: true }).selectOption("false");
    await managedInterface.check();
    for (const invalid of [".", "eth:0", "eth 0", "abcdefghijklmnop", "网卡接口名字"]) {
      await interfaceName.fill(invalid);
      await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
      await expect(form.getByRole("alert")).toContainText("出口网卡名称必须为空或最多 15 个 UTF-8 字节");
      expect(await api("settings")).toEqual(original);
    }
    await interfaceName.fill("");
    for (const invalid of ["-1", "4294967296", "1.5", "0xff", "abc"]) {
      await mark.fill(invalid);
      await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
      await expect(form.getByRole("alert")).toContainText("0–4294967295");
      expect(await api("settings")).toEqual(original);
    }
    await mark.fill("0");
    await expect(managedAgent).not.toBeChecked(); await expect(agent).toBeDisabled();
    await managedAgent.check();
    for (const invalid of ["非 ASCII", "x".repeat(1025)]) {
      await agent.fill(invalid);
      await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
      await expect(form.getByRole("alert")).toContainText("最多 1024 个可打印 ASCII 字符");
      expect(await api("settings")).toEqual(original);
    }
    await agent.fill(""); await etag.selectOption("false");
    await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
    await expect(form.getByRole("status")).toContainText("保存结果已核对");
    expect((await api("settings")).runtime).toEqual({ ...original.runtime, ipv6: false, "tcp-concurrent": false, "find-process-mode": "off", "keep-alive-interval": 0, "keep-alive-idle": -1, "disable-keep-alive": false, "interface-name": "", "routing-mark": 0, "global-ua": "", "etag-support": false });
    await expect(row).toContainText("服务设置：false");
    await expect(interfaceRow).toContainText('服务设置：""');
    await expect(agentRow).toContainText('服务设置：""');
    await expect(page.getByLabel("已保存核心下载设置", { exact: true })).toContainText('"etag-support": false');
    await expect(page.getByLabel("已保存出口设置", { exact: true })).toContainText('"interface-name": ""');
    await expect(panel).toContainText("内核未运行，实际值未确认");
    await page.getByRole("button", { name: "重新读取设置", exact: true }).click();
    await expect(tcp).toHaveValue("false"); await expect(mode).toHaveValue("off");
    await expect(interval).toHaveValue("0"); await expect(idle).toHaveValue("-1"); await expect(disable).toHaveValue("false");
    await expect(managedInterface).toBeChecked(); await expect(interfaceName).toHaveValue(""); await expect(mark).toHaveValue("0");
    await expect(managedAgent).toBeChecked(); await expect(agent).toHaveValue(""); await expect(etag).toHaveValue("false");
    await page.route("**/api/commands", async route => {
      if (route.request().postDataJSON().command === "set_settings") await route.fulfill({ status: 503, contentType: "application/json", body: JSON.stringify({ error: { message: "fixture settings failure" } }) });
      else await route.continue();
    });
    await interfaceName.fill("lo"); await mark.fill("123");
    await agent.fill("browser-agent/1"); await etag.selectOption("true");
    await interval.fill("20"); await idle.fill("40"); await disable.selectOption("true");
    await tcp.selectOption("true"); await mode.selectOption("always");
    await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
    await expect(form.getByRole("status")).toContainText("草稿已保留");
    await expect(tcp).toHaveValue("true"); await expect(mode).toHaveValue("always");
    await expect(interval).toHaveValue("20"); await expect(idle).toHaveValue("40"); await expect(disable).toHaveValue("true");
    await expect(interfaceName).toHaveValue("lo"); await expect(mark).toHaveValue("123");
    await expect(agent).toHaveValue("browser-agent/1"); await expect(etag).toHaveValue("true");
    expect((await api("settings")).runtime["global-ua"]).toBe("");
    expect((await api("settings")).runtime["etag-support"]).toBe(false);
    expect((await api("settings")).runtime["interface-name"]).toBe("");
    expect((await api("settings")).runtime["routing-mark"]).toBe(0);
    expect((await api("settings")).runtime["keep-alive-interval"]).toBe(0);
    expect((await api("settings")).runtime["tcp-concurrent"]).toBe(false);
    await page.unroute("**/api/commands");
    let fail = true;
    await page.route("**/api/commands", async route => {
      if (route.request().postDataJSON().command === "connection_settings") {
        if (fail) await route.fulfill({ status: 503, contentType: "application/json", body: JSON.stringify({ error: { message: "fixture connection read failure" } }) });
        else await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ running: true, config_revision: "fixture.yaml", error: null, fields: [{ key: "tcp-concurrent", setting: false, configured: false, actual: true, mismatch: true }, { key: "find-process-mode", setting: "off", configured: "off", actual: "always", mismatch: true }, { key: "interface-name", setting: "", configured: "", actual: "lo", mismatch: true }, { key: "routing-mark", setting: 0, configured: 0, actual: 123, mismatch: true }] }) });
      } else await route.continue();
    });
    await panel.getByRole("button", { name: "刷新连接设置读回", exact: true }).click();
    await expect(panel.getByRole("alert")).toContainText("fixture connection read failure");
    await expect(row).toHaveCount(0);
    fail = false;
    await panel.getByRole("button", { name: "刷新连接设置读回", exact: true }).click();
    await expect(row).toContainText("配置值与内核实际值不一致");
    await expect(interfaceRow).toContainText("配置值与内核实际值不一致");
    await page.unroute("**/api/commands");
    await panel.getByRole("button", { name: "刷新连接设置读回", exact: true }).click();
    await expect(panel).toContainText("内核未运行，实际值未确认");
    await expect(panel).not.toContainText("配置值与内核实际值不一致");
    await mark.fill("4294967295");
    await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
    await expect(form.getByRole("status")).toContainText("保存结果已核对");
    expect((await api("settings")).runtime["interface-name"]).toBe("lo");
    expect((await api("settings")).runtime["routing-mark"]).toBe(4294967295);
    await page.getByRole("button", { name: "重新读取设置", exact: true }).click();
    await expect(managedInterface).toBeChecked(); await expect(interfaceName).toHaveValue("lo"); await expect(mark).toHaveValue("4294967295");
    expect((await api("settings")).runtime["global-ua"]).toBe("browser-agent/1");
    expect((await api("settings")).runtime["etag-support"]).toBe(true);
    await expect(agent).toHaveValue("browser-agent/1"); await expect(etag).toHaveValue("true");
    await managedAgent.uncheck(); await etag.selectOption(""); await expect(agent).toBeDisabled();
    await managedInterface.uncheck(); await mark.fill("");
    await expect(interfaceName).toBeDisabled();
    await interval.fill(""); await idle.fill(""); await disable.selectOption("");
    await tcp.selectOption(""); await mode.selectOption("");
    await page.getByRole("combobox", { name: "IPv6", exact: true }).selectOption("");
    await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
    await expect(form.getByRole("status")).toContainText("保存结果已核对");
    expect((await api("settings")).runtime).toEqual(original.runtime);
    await page.getByRole("button", { name: "退出登录", exact: true }).click();
  } finally { await page.unroute("**/api/commands"); await api("set_settings", { runtime: original.runtime }); }
});

test("hosts editor preserves map shapes, explicit empty and false switches with failed drafts", async ({ page }) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, { method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" }, body: JSON.stringify({ command, ...fields }) });
    expect(response.ok).toBe(true); return response.json();
  };
  const original = await api("settings");
  await page.goto(`${base}/settings`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  const form = page.getByRole("region", { name: "服务设置编辑器", exact: true });
  const owned = page.getByRole("checkbox", { name: "管理 hosts 映射", exact: true });
  const hosts = page.getByRole("textbox", { name: "hosts JSON 映射", exact: true });
  const save = page.getByRole("button", { name: "保存服务设置", exact: true });
  const custom = { "*.example.test": "192.0.2.1", "multi.example.test": ["192.0.2.2", "2001:db8::1"], "alias.example.test": "multi.example.test" };
  try {
    await expect(owned).not.toBeChecked(); await expect(hosts).toBeDisabled();
    await owned.check();
    for (const invalid of ['[]', '{"a.test":123}', '{"a.test":[]}', '{"a.test":["alias.test"]}', '{"*.test":"a.test"}', '{"a.test":"192.0.2.1","A.TEST":"192.0.2.2"}']) {
      await hosts.fill(invalid); await save.click();
      await expect(form.getByRole("alert")).toContainText("hosts");
      expect(await api("settings")).toEqual(original);
    }
    await hosts.fill(JSON.stringify(custom));
    await page.getByRole("combobox", { name: "DNS 设置来源", exact: true }).selectOption("true");
    await page.getByRole("combobox", { name: "DNS 使用 hosts", exact: true }).selectOption("false");
    await page.getByRole("combobox", { name: "DNS 使用系统 hosts", exact: true }).selectOption("false");
    await save.click(); await expect(form.getByRole("status")).toContainText("保存结果已核对");
    expect((await api("settings")).runtime).toEqual({ ...original.runtime, hosts: custom, dns: { "use-hosts": false, "use-system-hosts": false } });
    await expect(page.getByLabel("已保存 hosts 映射", { exact: true })).toContainText('"alias.example.test": "multi.example.test"');
    await page.getByRole("button", { name: "重新读取设置", exact: true }).click();
    await expect(owned).toBeChecked(); expect(JSON.parse(await hosts.inputValue())).toEqual(custom);
    await expect(page.getByRole("combobox", { name: "DNS 使用系统 hosts", exact: true })).toHaveValue("false");
    await hosts.fill(JSON.stringify(Object.fromEntries(Object.entries(custom).reverse())));
    await expect(save).toBeDisabled();
    await page.route("**/api/commands", async route => {
      if (route.request().postDataJSON().command === "set_settings") await route.fulfill({ status: 503, contentType: "application/json", body: JSON.stringify({ error: { message: "fixture hosts failure" } }) });
      else await route.continue();
    });
    await hosts.fill('{}'); await save.click(); await expect(form.getByRole("status")).toContainText("草稿已保留");
    await expect(hosts).toHaveValue('{}'); expect((await api("settings")).runtime.hosts).toEqual(custom);
    await page.unroute("**/api/commands");
    await save.click(); await expect(form.getByRole("status")).toContainText("保存结果已核对");
    expect((await api("settings")).runtime.hosts).toEqual({});
    await page.getByRole("button", { name: "重新读取设置", exact: true }).click();
    await expect(owned).toBeChecked(); await expect(hosts).toHaveValue('{}');
    await page.getByRole("combobox", { name: "DNS 设置来源", exact: true }).selectOption("");
    await save.click(); await expect(form.getByRole("status")).toContainText("保存结果已核对");
    expect((await api("settings")).runtime).toEqual({ ...original.runtime, hosts: {} });
    await expect(page.getByRole("region", { name: "订阅 DNS 覆盖", exact: true })).not.toContainText("启用覆盖前");
    await owned.uncheck(); await expect(hosts).toBeDisabled();
    await save.click(); await expect(form.getByRole("status")).toContainText("保存结果已核对");
    expect((await api("settings")).runtime).toEqual(original.runtime);
    await expect(page.getByRole("region", { name: "订阅 DNS 覆盖", exact: true })).toContainText("DNS 配置段或 hosts 映射");
    await page.getByRole("button", { name: "退出登录", exact: true }).click();
  } finally { await page.unroute("**/api/commands"); await api("set_settings", { runtime: original.runtime }); }
});

test("Geo settings editor saves inherited URL leaves and reads core values with retry", async ({ page }) => {
  const api = async (command: string, fields: Record<string, unknown> = {}) => {
    const response = await fetch(`${base}/api/commands`, { method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" }, body: JSON.stringify({ command, ...fields }) });
    expect(response.ok).toBe(true);
    return response.json();
  };
  const original = await api("settings");
  await page.goto(`${base}/settings`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  const form = page.getByRole("region", { name: "服务设置编辑器", exact: true });
  const readback = page.getByRole("region", { name: "Geo 设置读回", exact: true });
  try {
    await page.getByRole("combobox", { name: "Geo 数据模式", exact: true }).selectOption("false");
    await page.getByRole("combobox", { name: "Geo 加载器", exact: true }).selectOption("standard");
    await page.getByRole("combobox", { name: "GeoSite 匹配器", exact: true }).selectOption("mph");
    await page.getByRole("combobox", { name: "Geo 自动更新", exact: true }).selectOption("false");
    await page.getByRole("textbox", { name: "Geo 更新间隔（小时）", exact: true }).fill("0");
    await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
    await expect(form.getByRole("alert")).toContainText("1–8760");
    expect(await api("settings")).toEqual(original);
    await page.getByRole("textbox", { name: "Geo 更新间隔（小时）", exact: true }).fill("48");
    await page.getByRole("checkbox", { name: "管理 Geo 下载地址", exact: true }).check();
    await page.getByRole("textbox", { name: "MMDB 下载地址", exact: true }).fill("file:///etc/passwd");
    await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
    await expect(form.getByRole("alert")).toContainText("HTTP(S)");
    expect(await api("settings")).toEqual(original);
    await page.getByRole("textbox", { name: "MMDB 下载地址", exact: true }).fill("http://127.0.0.1:1/browser-mmdb");
    await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
    await expect(form.getByRole("status")).toContainText("保存结果已核对");
    const saved = await api("settings");
    expect(saved.runtime["geodata-mode"]).toBe(false);
    expect(saved.runtime["geo-auto-update"]).toBe(false);
    expect(saved.runtime["geodata-loader"]).toBe("standard");
    expect(saved.runtime["geosite-matcher"]).toBe("mph");
    const matcher = readback.locator("li").filter({ has: page.getByText("geosite-matcher", { exact: true }) });
    await expect(matcher).toContainText("服务设置：mph");
    await expect(matcher).toContainText("内核实际值：未确认");
    expect(saved.runtime["geo-update-interval"]).toBe(48);
    expect(saved.runtime["geox-url"]).toEqual({ mmdb: "http://127.0.0.1:1/browser-mmdb" });
    const mmdb = readback.locator("li").filter({ has: page.getByText("geox-url.mmdb", { exact: true }) });
    await expect(mmdb).toContainText("服务设置：http://127.0.0.1:1/browser-mmdb");
    await expect(readback).toContainText("内核未运行，实际值未确认");
    await page.getByRole("button", { name: "重新读取设置", exact: true }).click();
    await expect(page.getByRole("textbox", { name: "MMDB 下载地址", exact: true })).toHaveValue("http://127.0.0.1:1/browser-mmdb");
    await expect(page.getByRole("combobox", { name: "GeoSite 匹配器", exact: true })).toHaveValue("mph");
    let fail = true;
    await page.route("**/api/commands", async route => {
      if (route.request().postDataJSON().command === "geo_settings") {
        if (fail) await route.fulfill({ status: 503, contentType: "application/json", body: JSON.stringify({ error: { message: "fixture Geo read failed" } }) });
        else await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ config_revision: "fixture.yaml", running: true, error: null, fields: [{ key: "geox-url.mmdb", setting: "http://127.0.0.1:1/browser-mmdb", configured: "http://127.0.0.1:1/browser-mmdb", actual: "http://127.0.0.1:1/old", mismatch: true }, { key: "geosite-matcher", setting: "mph", configured: "mph", actual: "succinct", mismatch: true }] }) });
      } else await route.continue();
    });
    await readback.getByRole("button", { name: "刷新 Geo 设置读回" }).click();
    await expect(readback.getByRole("alert")).toContainText("fixture Geo read failed");
    await expect(mmdb).toHaveCount(0);
    await expect(matcher).toHaveCount(0);
    fail = false;
    await readback.getByRole("button", { name: "刷新 Geo 设置读回" }).click();
    await expect(matcher).toContainText("配置值与内核实际值不一致");
    await expect(matcher).toContainText("内核实际值：succinct");
    await expect(mmdb).toContainText("内核实际值：http://127.0.0.1:1/old");
    await page.unroute("**/api/commands");
    await readback.getByRole("button", { name: "刷新 Geo 设置读回" }).click();
    await expect(readback).toContainText("内核未运行，实际值未确认");
    await expect(readback).not.toContainText("配置值与内核实际值不一致");
    await page.getByRole("combobox", { name: "Geo 数据模式", exact: true }).selectOption("");
    await page.getByRole("combobox", { name: "Geo 自动更新", exact: true }).selectOption("");
    await page.getByRole("combobox", { name: "Geo 加载器", exact: true }).selectOption("");
    await page.getByRole("combobox", { name: "GeoSite 匹配器", exact: true }).selectOption("");
    await page.getByRole("textbox", { name: "Geo 更新间隔（小时）", exact: true }).fill("");
    await page.getByRole("checkbox", { name: "管理 Geo 下载地址", exact: true }).uncheck();
    await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
    await expect(form.getByRole("status")).toContainText("保存结果已核对");
    expect((await api("settings")).runtime).toEqual(original.runtime);
    await page.getByRole("button", { name: "退出登录", exact: true }).click();
  } finally {
    await page.unroute("**/api/commands");
    await api("set_settings", { runtime: original.runtime });
  }
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
  await expect(page.locator("header")).toContainText("启动失败");
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
  await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
  await expect(
    page.getByText("保存结果已核对，服务设置与提交内容一致。", { exact: true }),
  ).toBeVisible();
  await expect(page.locator("header")).toContainText("启动失败");
  await page.getByRole("button", { name: "全部改为继承", exact: true }).click();
  await page.getByRole("button", { name: "确认改为继承", exact: true }).click();
  await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
  await expect(
    page.getByText("保存结果已核对，服务设置与提交内容一致。", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /订阅/ })
    .click();
  const yaml =
    "mixed-port: 0\nmode: rule\nlog-level: info\nexternal-controller: ''\ndns: {enable: false}\ntun: {enable: false}\nprofile: {store-selected: false}\nproxy-groups:\n  - {name: Main, type: select, proxies: [DIRECT, REJECT]}\nrules: ['MATCH,Main']\n";
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
  await page
    .getByLabel("订阅链接", { exact: true })
    .fill(`${subscriptionUrl}/error`);
  await page.getByRole("button", { name: "下载并导入", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("503");
  await expect(page.locator("article.profile")).toHaveCount(2);
  await page.getByRole("button", { name: "启动内核" }).click();
  await expect(page.locator("header")).toContainText("运行中");
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /节点/ })
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
  await expect(page.locator("header")).toContainText("运行中");
  await page
    .getByLabel("运行配置 YAML")
    .fill(yaml.replace("mode: rule", "mode: direct"));
  await page.getByRole("button", { name: "校验并应用" }).click();
  await expect(page.getByText("此配置已通过校验并提交。")).toBeVisible();
  await page.getByRole("button", { name: "停止内核" }).click();
  await expect(page.locator("header")).toContainText("已停止");
  await expect(page.getByLabel("运行配置 YAML")).toBeEnabled();
  await page.getByRole("button", { name: "重启内核" }).click();
  await expect(page.locator("header")).toContainText("运行中");
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
    .getByRole("link", { name: /节点/ })
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
  await expect(
    page.getByRole("heading", { name: "连接你的服务" }),
  ).toBeVisible();
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务" }).click();
  await expect(page.getByLabel("运行配置 YAML")).toBeVisible();
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
  expect(errors).toEqual([]);
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
    .getByRole("link", { name: /节点/ })
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
  await expect(page.locator("header")).toContainText("运行中");
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
    .getByRole("link", { name: /节点/ })
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
  await expect(
    edited.getByRole("button", { name: "删除订阅 EditedRemote", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "停止内核", exact: true }).click();
  await expect(page.locator("header")).toContainText("已停止");
  await expect(
    edited.getByRole("button", { name: "删除订阅 EditedRemote", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "启动内核", exact: true }).click();
  await expect(page.locator("header")).toContainText("运行中");
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
  const local = page.locator("article.profile").filter({
    has: page.getByRole("heading", { name: "Browser", exact: true }),
  });
  await page
    .getByRole("button", { name: "删除订阅 Browser", exact: true })
    .click();
  await page.getByRole("button", { name: "取消删除", exact: true }).click();
  await expect(page.locator("article.profile")).toHaveCount(2);
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
  await expect(page.locator("header")).toContainText("运行中");
  await navigate(/配置/);
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: global/);
  await navigate(/订阅/);
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
  await expect(page.locator("header")).toContainText("运行中");
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
  await navigate(/节点/);
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
  await navigate(/节点/);
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
  await expect(page.locator("header")).toContainText("运行中");
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
    await expect(page.getByRole("alert")).toBeVisible();
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
  await expect(page.locator("header")).toContainText("运行中");
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
  await expect(page.locator("header")).toContainText("运行中");
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
  expect((await api("global_script")).source).toBe(defaultScript);
  await page.getByRole("button", { name: "停止内核", exact: true }).click();
  await expect(page.locator("header")).toContainText("已停止");
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
  await page.getByRole("button", { name: "启动内核", exact: true }).click();
  await expect(page.locator("header")).toContainText("运行中");
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
  await page.getByRole("button", { name: "停止内核", exact: true }).click();
  await expect(page.locator("header")).toContainText("已停止");
  await api("set_settings", { runtime: { mode: "rule", "mixed-port": 0 } });
  await reopenConfig();
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: rule/);
  expect((await api("status")).pid).toBeNull();
  await api("set_settings", { runtime: {} });
  await reopenConfig();
  await expect(page.getByLabel("运行配置 YAML")).toHaveValue(/mode: direct/);
  await page.getByRole("button", { name: "启动内核", exact: true }).click();
  await expect(page.locator("header")).toContainText("运行中");
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

test("settings editor preserves inheritance, failed drafts and uncertain saves with confirmed reads", async ({
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
  const input = (name: string) =>
    page.getByRole("textbox", { name, exact: true });
  const select = (name: string) =>
    page.getByRole("combobox", { name, exact: true });
  const save = page.getByRole("button", { name: "保存服务设置", exact: true });
  const summary = page.getByRole("region", {
    name: "已保存服务设置",
    exact: true,
  });
  const failRead = async (route: import("@playwright/test").Route) => {
    if (route.request().postDataJSON()?.command === "settings")
      await route.fulfill({
        status: 503,
        contentType: "application/json",
        body: JSON.stringify({
          error: { message: "fixture settings read failed" },
        }),
      });
    else await route.continue();
  };
  await page.route("**/api/commands", failRead);
  await page.goto(`${base}/settings`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "fixture settings read failed",
  );
  await expect(page.getByRole("form", { name: "运行设置表单" })).toHaveCount(0);
  await page.unroute("**/api/commands", failRead);
  await page.getByRole("button", { name: "重试读取设置", exact: true }).click();
  await expect(save).toBeDisabled();
  await expect(select("代理模式")).toHaveValue("");
  const uid = (await api("status")).active_profile;
  const catalog = await api("profiles");
  const baseItem = catalog.items.find(
    (item: { uid: string }) => item.uid === uid,
  );
  const raw = await readFile(
    join(directory, "profiles", baseItem.file),
    "utf8",
  );
  for (const name of [
    "混合端口",
    "SOCKS 端口",
    "HTTP 端口",
    "重定向端口",
    "透明代理端口",
  ])
    await input(name).fill("0");
  await select("代理模式").selectOption("global");
  for (const name of ["允许局域网访问", "IPv6", "统一延迟"])
    await select(name).selectOption("false");
  await select("日志等级").selectOption("warning");
  await save.click();
  await expect(
    page.getByText("保存结果已核对，服务设置与提交内容一致。", { exact: true }),
  ).toBeVisible();
  const saved = await api("settings");
  expect(saved.runtime).toEqual({
    "mixed-port": 0,
    "socks-port": 0,
    port: 0,
    "redir-port": 0,
    "tproxy-port": 0,
    mode: "global",
    "allow-lan": false,
    ipv6: false,
    "unified-delay": false,
    "log-level": "warning",
  });
  await expect(summary).toContainText("禁用");
  expect((await api("config")).yaml).toMatch(/mode: global/);
  expect(
    await readFile(join(directory, "profiles", baseItem.file), "utf8"),
  ).toBe(raw);
  await page.screenshot({
    path: "test-results/settings-editor-desktop.png",
    fullPage: true,
  });
  let invalidSent = 0;
  const inspect = async (route: import("@playwright/test").Route) => {
    if (route.request().postDataJSON()?.command === "set_settings")
      invalidSent++;
    await route.continue();
  };
  await page.route("**/api/commands", inspect);
  await input("混合端口").fill("70000");
  await save.click();
  await expect(page.getByRole("alert")).toContainText(
    "混合端口必须是 0–65535 的整数",
  );
  expect(invalidSent).toBe(0);
  await page.unroute("**/api/commands", inspect);
  await input("混合端口").fill("0");
  await select("代理模式").selectOption("direct");
  await page.getByRole("button", { name: "重新读取设置", exact: true }).click();
  await expect(page.getByRole("group", { name: "设置替换确认" })).toBeVisible();
  await page.getByRole("button", { name: "继续编辑设置", exact: true }).click();
  await expect(select("代理模式")).toHaveValue("direct");
  expect(await api("settings")).toEqual(saved);
  await page.getByRole("button", { name: "重新读取设置", exact: true }).click();
  await page.getByRole("button", { name: "确认重新读取", exact: true }).click();
  await expect(select("代理模式")).toHaveValue("global");
  await api("set_profile_script", {
    uid,
    source:
      "function main(c) { if(c.mode==='rule') c.rules=['INVALID,DIRECT']; return c; }",
  });
  const before = await api("status");
  await select("代理模式").selectOption("rule");
  await save.click();
  await expect(page.getByRole("alert").first()).toContainText(
    "Mihomo rejected",
  );
  await expect(
    page.getByText(
      "服务当前设置与提交内容不同，草稿已保留。请检查错误或重新读取设置。",
      { exact: true },
    ),
  ).toBeVisible();
  await expect(select("代理模式")).toHaveValue("rule");
  await expect(save).toBeEnabled();
  expect(await api("settings")).toEqual(saved);
  expect((await api("status")).config_revision).toBe(before.config_revision);
  expect((await api("status")).pid).toBe(before.pid);
  await api("clear_profile_script", { uid });
  await select("代理模式").selectOption("global");
  await select("统一延迟").selectOption("true");
  await page.route("**/api/commands", failRead);
  await save.click();
  await expect(page.getByText(/保存结果尚未核对/)).toBeVisible();
  await expect(save).toBeDisabled();
  await expect(select("统一延迟")).toHaveValue("true");
  expect((await api("settings")).runtime["unified-delay"]).toBe(true);
  await page.unroute("**/api/commands", failRead);
  await page
    .getByRole("button", { name: "核对已保存设置", exact: true })
    .click();
  await expect(
    page.getByText("已核对：服务已保存当前草稿。", { exact: true }),
  ).toBeVisible();
  await expect(save).toBeDisabled();
  // An error response after actual publication must be reconciled, never assumed rollback.
  const lostReply = async (route: import("@playwright/test").Route) => {
    if (route.request().postDataJSON()?.command === "set_settings") {
      const response = await route.fetch();
      expect(response.ok()).toBe(true);
      await route.fulfill({
        status: 500,
        contentType: "application/json",
        body: JSON.stringify({
          error: { message: "fixture commit response lost" },
        }),
      });
    } else await route.continue();
  };
  await page.route("**/api/commands", lostReply);
  await select("IPv6").selectOption("true");
  await save.click();
  await expect(
    page.getByText("请求报告错误，但服务已保存此草稿，已核对，无需重复提交。", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(page.getByRole("alert").first()).toContainText(
    "fixture commit response lost",
  );
  await expect(select("IPv6")).toHaveValue("true");
  await expect(save).toBeDisabled();
  await page.unroute("**/api/commands", lostReply);
  await page.getByRole("button", { name: "停止内核", exact: true }).click();
  await expect(page.locator("header")).toContainText("已停止");
  await page
    .getByRole("button", { name: "核对已保存设置", exact: true })
    .click();
  await expect(
    page.getByText("已核对：服务已保存当前草稿。", { exact: true }),
  ).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: "test-results/settings-editor-mobile.png",
    fullPage: true,
  });
  const beforeClear = await api("settings");
  await page.getByRole("button", { name: "全部改为继承", exact: true }).click();
  await page.getByRole("button", { name: "继续编辑设置", exact: true }).click();
  expect(await api("settings")).toEqual(beforeClear);
  await page.getByRole("button", { name: "全部改为继承", exact: true }).click();
  await page.getByRole("button", { name: "确认改为继承", exact: true }).click();
  await expect(select("代理模式")).toHaveValue("");
  expect(await api("settings")).toEqual(beforeClear);
  await save.click();
  await expect(
    page.getByText("保存结果已核对，服务设置与提交内容一致。", { exact: true }),
  ).toBeVisible();
  expect((await api("settings")).runtime).toEqual({});
  expect((await api("status")).pid).toBeNull();
  expect((await api("config")).yaml).toMatch(/mode: direct/);
  // Standalone authority release retains the current runtime value, as documented.
  await api("apply_config", { yaml: "mode: direct\n" });
  await select("代理模式").selectOption("global");
  await save.click();
  await expect(
    page.getByText("保存结果已核对，服务设置与提交内容一致。", { exact: true }),
  ).toBeVisible();
  await select("代理模式").selectOption("");
  await save.click();
  await expect(
    page.getByText("保存结果已核对，服务设置与提交内容一致。", { exact: true }),
  ).toBeVisible();
  expect((await api("settings")).runtime).toEqual({});
  expect((await api("config")).yaml).toMatch(/mode: global/);
  await api("select_profile", { uid });
  await page.getByRole("button", { name: "启动内核", exact: true }).click();
  await expect(page.locator("header")).toContainText("运行中");
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /概览/ })
    .click();
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("link", { name: /设置/ })
    .click();
  await expect(select("代理模式")).toHaveValue("");
  expect((await api("settings")).runtime).toEqual({});
  // An unsupported response is never converted into an empty replacement form.
  const unknown = async (route: import("@playwright/test").Route) => {
    if (route.request().postDataJSON()?.command === "settings")
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          schema_version: 1,
          runtime: { mode: "global", "future-setting": true },
        }),
      });
    else await route.continue();
  };
  await select("代理模式").selectOption("global");
  await expect(save).toBeEnabled();
  await page.route("**/api/commands", unknown);
  await page.getByRole("button", { name: "重新读取设置", exact: true }).click();
  await page.getByRole("button", { name: "确认重新读取", exact: true }).click();
  await expect(page.getByRole("alert").last()).toContainText(
    "暂不支持的设置 future-setting",
  );
  await expect(save).toBeDisabled();
  expect((await api("settings")).runtime).toEqual({});
  await page.unroute("**/api/commands", unknown);
  await page
    .getByRole("button", { name: "核对已保存设置", exact: true })
    .click();
  await expect(
    page.getByText(
      "已核对：服务当前设置与草稿不同，草稿已保留。请修正或重新读取后再保存。",
      { exact: true },
    ),
  ).toBeVisible();
  await expect(select("代理模式")).toHaveValue("global");
  await expect(save).toBeEnabled();
  await page.getByRole("button", { name: "重新读取设置", exact: true }).click();
  await page.getByRole("button", { name: "确认重新读取", exact: true }).click();
  await expect(select("代理模式")).toHaveValue("");
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
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
  await expect(
    page.getByRole("textbox", { name: "DNS 解析服务器", exact: true }),
  ).toHaveValue('["1.1.1.1"]');
  await page
    .getByRole("combobox", { name: "代理模式", exact: true })
    .selectOption("direct");
  await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
  await expect(
    page.getByText("保存结果已核对，服务设置与提交内容一致。", { exact: true }),
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
  await expect(
    page.getByRole("textbox", { name: "DNS 解析服务器", exact: true }),
  ).toHaveValue('["1.1.1.1"]');
  await api("set_settings", { runtime: {} });
  await page.getByRole("button", { name: "重新读取设置", exact: true }).click();
  await expect(page.getByRole("form", { name: "运行设置表单" })).toBeVisible();
  expect((await api("settings")).profile_dns[uid].enabled).toBe(false);
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
});

test("network editor preserves all supported fields and explicitly confirms changing provider DNS", async ({
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
  const select = (name: string) =>
    page.getByRole("combobox", { name, exact: true });
  const input = (name: string) =>
    page.getByRole("textbox", { name, exact: true });
  const save = page.getByRole("button", { name: "保存服务设置", exact: true });
  const panel = page.getByRole("region", {
    name: "订阅 DNS 覆盖",
    exact: true,
  });
  const enable = panel.getByRole("button", {
    name: "启用订阅 DNS 覆盖",
    exact: true,
  });
  const confirm = panel.getByRole("button", {
    name: "确认覆盖当前订阅 DNS",
    exact: true,
  });
  await api("stop");
  subscriptionBody =
    "proxies: []\nmode: direct\nmixed-port: 0\ndns: {enable: false, nameserver: [9.9.9.9], nameserver-policy: {example.org: 8.8.8.8}}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']";
  const profile = await api("import_remote_profile", {
    url: subscriptionUrl,
    name: "Network editor provider",
  });
  const uid = profile.uid;
  await api("select_profile", { uid });
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
  await api("set_settings", { runtime });
  await page.goto(`${base}/settings`);
  await page.getByLabel("管理令牌").fill(token);
  await page.getByRole("button", { name: "连接服务", exact: true }).click();
  await expect(input("DNS 解析服务器")).toHaveValue('["1.1.1.1"]');
  await expect(select("DNS Fake-IP 过滤模式")).toHaveValue("whitelist");
  await expect(select("DNS 优先 HTTP/3")).toHaveValue("true");
  await expect(select("DNS 遵循代理规则")).toHaveValue("true");
  await expect(input("DNS 域名解析策略")).toHaveValue('{"owned.test":["1.1.1.1"]}');
  await expect(input("DNS 后备过滤条件")).toHaveValue('{"geoip":false,"geoip-code":"CN"}');
  await expect(input("DNS 监听地址")).toHaveValue("");
  await expect(select("DNS 监听地址来源")).toHaveValue("true");
  await expect(input("TUN DNS 劫持列表")).toHaveValue("[]");
  await expect(save).toBeDisabled();
  await expect(enable).toBeEnabled();
  const before = (await api("status")).config_revision;
  await enable.click();
  await expect(confirm).toBeVisible();
  expect((await api("status")).config_revision).toBe(before);
  await panel.getByRole("button", { name: "取消 DNS 确认" }).click();
  await expect(confirm).toHaveCount(0);
  expect((await api("profile_dns", { uid })).enabled).toBe(false);
  await enable.click();
  await confirm.click();
  await expect(panel.getByText("允许", { exact: true })).toBeVisible();
  expect((await api("config")).yaml).toContain("1.1.1.1");
  await input("TUN MTU").fill("0");
  await expect(enable).toBeDisabled();
  await save.click();
  await expect(page.getByRole("alert").last()).toContainText(
    "TUN MTU 必须是 1–65535",
  );
  expect((await api("settings")).runtime).toEqual(runtime);
  await input("TUN MTU").fill("1400");
  await input("DNS 后备解析服务器").fill("[1]");
  await save.click();
  await expect(page.getByRole("alert").last()).toContainText(
    "必须是 JSON 字符串列表",
  );
  await input("DNS 后备解析服务器").fill("[]");
  await input("DNS 域名解析策略").fill('{"bad.test":[]}');
  await save.click();
  await expect(page.getByRole("alert").last()).toContainText("必须是有效的 JSON 对象");
  expect((await api("settings")).runtime).toEqual(runtime);
  await input("DNS 域名解析策略").fill('{"owned.test":["1.1.1.1"]}');
  await select("DNS 启用").selectOption("true");
  await input("DNS 解析服务器").fill('["https://["]');
  const prior = await api("status");
  await save.click();
  await expect(
    page.getByText(
      "服务当前设置与提交内容不同，草稿已保留。请检查错误或重新读取设置。",
      { exact: true },
    ),
  ).toBeVisible();
  await expect(input("DNS 解析服务器")).toHaveValue('["https://["]');
  expect((await api("status")).config_revision).toBe(prior.config_revision);
  expect((await api("settings")).runtime).toEqual(runtime);
  await input("DNS 解析服务器").fill('["1.1.1.1"]');
  await select("DNS 启用").selectOption("false");
  await save.click();
  await expect(
    page.getByText("保存结果已核对，服务设置与提交内容一致。", { exact: true }),
  ).toBeVisible();
  expect((await api("settings")).runtime).toEqual({
    ...runtime,
    tun: { ...runtime.tun, mtu: 1400 },
  });
  expect((await api("settings")).profile_dns[uid].enabled).toBe(true);
  expect((await api("status")).pid).toBeNull();
  // A protected source refresh invalidates permission; it must never auto-confirm.
  subscriptionBody = subscriptionBody.replace("8.8.8.8", "8.8.4.4");
  await api("refresh_profile", { uid });
  await expect(panel.getByText("未允许", { exact: true })).toBeVisible();
  await expect(confirm).toHaveCount(0);
  await enable.click();
  await expect(confirm).toBeVisible();
  const challenge = (await api("profile_dns", { uid })).source;
  // A second refresh while the question is open cancels the old question.
  subscriptionBody = subscriptionBody.replace("8.8.4.4", "8.8.8.8");
  await api("refresh_profile", { uid });
  await expect(confirm).toHaveCount(0);
  expect((await api("profile_dns", { uid })).source).not.toBe(challenge);
  expect((await api("profile_dns", { uid })).enabled).toBe(false);
  await enable.click();
  await confirm.click();
  await expect(panel.getByText("允许", { exact: true })).toBeVisible();
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  await expect(panel.getByText("未允许", { exact: true })).toBeVisible();
  await expect(panel).toContainText("已提交的运行配置可能仍保留原值");
  expect((await api("config")).yaml).toContain("1.1.1.1");
  await enable.click();
  await expect(confirm).toBeVisible();
  const other = await api("import_profile", {
    name: "Other DNS provider",
    yaml: subscriptionBody,
  });
  await api("select_profile", { uid: other.uid });
  await expect(panel).toContainText(other.uid);
  await expect(confirm).toHaveCount(0);
  // Unknown nested fields fail closed while retaining a dirty network draft.
  await input("TUN MTU").fill("1300");
  const unsupported = async (route: import("@playwright/test").Route) => {
    if (route.request().postDataJSON()?.command !== "settings")
      return route.continue();
    const response = await route.fetch();
    const value = await response.json();
    value.runtime.tun["future-option"] = true;
    await route.fulfill({ response, json: value });
  };
  await page.route("**/api/commands", unsupported);
  await page
    .getByRole("button", { name: "核对已保存设置", exact: true })
    .click();
  await expect(page.getByRole("alert").last()).toContainText(
    "暂不支持的设置 tun.future-option",
  );
  await expect(input("TUN MTU")).toHaveValue("1300");
  await expect(save).toBeDisabled();
  await page.unroute("**/api/commands", unsupported);
  await page
    .getByRole("button", { name: "核对已保存设置", exact: true })
    .click();
  await expect(save).toBeEnabled();
  await input("TUN MTU").fill("1400");
  await select("DNS 设置来源").selectOption("");
  await save.click();
  await expect(
    page.getByText("保存结果已核对，服务设置与提交内容一致。", { exact: true }),
  ).toBeVisible();
  expect((await api("settings")).runtime).toEqual({
    mode: "direct",
    "mixed-port": 0,
    tun: { ...runtime.tun, mtu: 1400 },
  });
  // Removing a section and restoring its local fields remains explicit.
  await select("DNS 设置来源").selectOption("true");
  await input("DNS 解析服务器").fill('["1.1.1.1"]');
  await save.click();
  await expect(
    page.getByText("保存结果已核对，服务设置与提交内容一致。", { exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: "test-results/network-settings-desktop.png",
    fullPage: true,
  });
  await page.setViewportSize({ width: 390, height: 844 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: "test-results/network-settings-mobile.png",
    fullPage: true,
  });
  await page.getByRole("button", { name: "退出登录", exact: true }).click();
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
  const save = page.getByRole("button", { name: "保存服务设置", exact: true });
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
  await page
    .getByRole("combobox", { name: "允许局域网访问", exact: true })
    .selectOption("false");
  await save.click();
  await expect(
    page.getByText("保存结果已核对，服务设置与提交内容一致。", { exact: true }),
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
  await save.click();
  await expect(
    page.getByText("保存结果已核对，服务设置与提交内容一致。", { exact: true }),
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
      editor.getByText(
        "服务原始订阅与提交内容不同，草稿已保留。请检查错误；版本变化时需重新读取。",
        { exact: true },
      ),
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
    editor.getByText("原始订阅保存结果已核对。", { exact: true }),
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
  await expect(
    editor.getByText(
      "请求报告错误，但服务已保存此草稿，已核对，无需重复提交。",
      { exact: true },
    ),
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
    editor.getByText("已核对：服务已保存当前原始订阅草稿。", { exact: true }),
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
    editor.getByText("原始订阅保存结果已核对。", { exact: true }),
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
      name: "编辑原始订阅 Raw editor provider",
      exact: true,
    })
    .click();
  await expect(input).toHaveValue(draft);
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
  const portHint = page.locator("#port-hint-port");
  await expect(input).toHaveValue("");
  await expect(input).toHaveAttribute("placeholder", `继承当前端口 ${first}`);
  await expect(portHint).toHaveText(`当前端口：${first} · 继承订阅 / 配置`);
  await expect(page.locator("#port-hint-mixed-port")).toContainText(
    "禁用（0）",
  );
  await input.fill(String(second));
  // A draft must not overwrite the displayed runtime or imply it is already live.
  await expect(portHint).toContainText(`当前端口：${first}`);
  await page.getByRole("button", { name: "刷新端口信息", exact: true }).click();
  await expect(portHint).toContainText(`当前端口：${first}`);
  await expect(input).toHaveValue(String(second));
  expect((await api("settings")).runtime).toEqual({});
  await page.getByRole("button", { name: "保存服务设置", exact: true }).click();
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
  await expect(portHint).toHaveText(
    `配置端口：${second} · 服务设置 · 内核未运行`,
  );
  await expect(input).toHaveValue(String(second));
  await api("start");
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
  await expect(
    page.getByRole("button", {
      name: "删除订阅 Cascade retained",
      exact: true,
    }),
  ).toBeDisabled();
  await stop();
  await start();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible({
    timeout: 15000,
  });
  await expect(card).toHaveCount(0);
  expect((await api("settings")).profile_dns[deleted.uid]).toBeUndefined();
  await expect(
    page.getByRole("button", {
      name: "删除订阅 Cascade retained",
      exact: true,
    }),
  ).toBeDisabled();
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
      panel.getByText(`已是最新${label} ${version}，无需重新安装。`, {
        exact: true,
      }),
    ).toBeVisible();
    await page.getByLabel("升级通道").selectOption(alpha ? "stable" : "alpha");
    await expect(page.getByText(`已是最新${label} ${version}，无需重新安装。`, { exact: true })).toHaveCount(0);
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
      panel.getByText(`已是最新${label} ${version}，无需重新安装。`, {
        exact: true,
      }),
    ).toHaveCount(0);
    await expect(forced).toBeEnabled();
    fail = false;
    page.once("dialog", (dialog) => dialog.accept());
    await forced.click();
    await expect(
      panel.getByText(`升级完成：${version} → ${version}`, { exact: true }),
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
      panel.getByText(`升级完成：${version} → ${version}`, { exact: true }),
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
    await expect(panel.getByText(`修复完成：${version}`, { exact: true })).toBeVisible();
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
