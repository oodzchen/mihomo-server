"use strict";
// Local status page: detect, install or start this user's mihomo-server service.
const invoke = (command, args) => window.__TAURI_INTERNALS__.invoke(command, args);

const TEXT = {
  en: {
    detecting: "Looking for the mihomo-server service on this computer…",
    not_installed: "The mihomo-server service is not installed on this computer.",
    installing: "Installing the service. Approve the system authorization prompt.",
    inactive: "The mihomo-server service is installed but not running.",
    starting: "Starting the service…",
    stopping: "Stopping the service…",
    restarting: "Restarting the service…",
    unreachable: "The mihomo-server service is not responding.",
    running: "The mihomo-server service is running",
    install: "Install mihomo-server service", start: "Start service", restart: "Restart service",
    open: "Open dashboard", wait: "Please wait…", busy_installing: "Installing…",
    busy_starting: "Starting…", busy_stopping: "Stopping…", busy_restarting: "Restarting…",
    failed_installing: "Installation did not finish. See the output below.",
    failed_starting: "The service did not start. See the output below.",
    failed_stopping: "The service did not stop. See the output below.",
    failed_restarting: "The service did not restart. See the output below.",
    show: "Show output", hide: "Hide output", empty: "No output yet.", output: "Output",
    settings: "Settings", back: "Back", language: "Interface language",
    language_hint: "Applies everywhere: the management page and the tray menu switch too.",
    language_failed: "The language did not change: {detail}",
    proxy: "Temporary proxy", test: "Test", testing: "Testing…",
    proxy_hint: "Only used to download the installer and the service. Not saved, and not needed after installation. Leave empty to connect directly.",
    proxy_set: "Installation will download through this proxy.",
    proxy_ok: "Connected through the proxy · {ms} ms",
    direct_ok: "Connected without a proxy · {ms} ms",
    proxy_failed: "Cannot reach GitHub: {detail}",
    invalid_proxy: "Invalid proxy address. Examples: http://127.0.0.1:7890, socks5://127.0.0.1:7891",
  },
  zh: {
    detecting: "正在检测本机的 mihomo-server 服务…",
    not_installed: "本机尚未安装 mihomo-server 服务",
    installing: "正在安装服务，请在系统弹出的授权对话框中确认",
    inactive: "mihomo-server 服务已安装，但未运行",
    starting: "正在启动服务…",
    stopping: "正在停止服务…",
    restarting: "正在重启服务…",
    unreachable: "mihomo-server 服务无响应",
    running: "mihomo-server 服务运行中",
    install: "安装 mihomo-server 服务", start: "启动服务", restart: "重启服务",
    open: "打开管理程序", wait: "请稍候…", busy_installing: "正在安装…",
    busy_starting: "正在启动…", busy_stopping: "正在停止…", busy_restarting: "正在重启…",
    failed_installing: "安装未完成，详情请查看下方输出",
    failed_starting: "服务启动失败，详情请查看下方输出",
    failed_stopping: "服务停止失败，详情请查看下方输出",
    failed_restarting: "服务重启失败，详情请查看下方输出",
    show: "显示输出", hide: "收起输出", empty: "暂无输出", output: "输出",
    settings: "设置", back: "返回", language: "界面语言",
    language_hint: "全局生效，管理界面和托盘菜单同步切换",
    language_failed: "语言未能修改：{detail}",
    proxy: "临时代理", test: "测试", testing: "正在测试…",
    proxy_hint: "仅用于下载安装脚本和服务程序，不会保存，安装完成后无需再使用；留空则直接连接",
    proxy_set: "安装时将通过此代理下载",
    proxy_ok: "代理连接正常 · {ms} ms",
    direct_ok: "无需代理即可连接 · {ms} ms",
    proxy_failed: "无法连接 GitHub：{detail}",
    invalid_proxy: "代理地址无效，示例：http://127.0.0.1:7890、socks5://127.0.0.1:7891",
  },
  zhtw: {
    detecting: "正在偵測本機的 mihomo-server 服務…",
    not_installed: "本機尚未安裝 mihomo-server 服務",
    installing: "正在安裝服務，請在系統彈出的授權對話框中確認",
    inactive: "mihomo-server 服務已安裝，但未執行",
    starting: "正在啟動服務…",
    stopping: "正在停止服務…",
    restarting: "正在重新啟動服務…",
    unreachable: "mihomo-server 服務無回應",
    running: "mihomo-server 服務執行中",
    install: "安裝 mihomo-server 服務", start: "啟動服務", restart: "重新啟動服務",
    open: "開啟管理程式", wait: "請稍候…", busy_installing: "正在安裝…",
    busy_starting: "正在啟動…", busy_stopping: "正在停止…", busy_restarting: "正在重新啟動…",
    failed_installing: "安裝未完成，詳情請查看下方輸出",
    failed_starting: "服務啟動失敗，詳情請查看下方輸出",
    failed_stopping: "服務停止失敗，詳情請查看下方輸出",
    failed_restarting: "服務重新啟動失敗，詳情請查看下方輸出",
    show: "顯示輸出", hide: "收起輸出", empty: "尚無輸出", output: "輸出",
    settings: "設定", back: "返回", language: "介面語言",
    language_hint: "全域生效，管理介面和系統匣選單同步切換",
    language_failed: "語言未能修改：{detail}",
    proxy: "臨時代理", test: "測試", testing: "正在測試…",
    proxy_hint: "僅用於下載安裝腳本和服務程式，不會儲存，安裝完成後無需再使用；留空則直接連線",
    proxy_set: "安裝時將透過此代理下載",
    proxy_ok: "代理連線正常 · {ms} ms",
    direct_ok: "無需代理即可連線 · {ms} ms",
    proxy_failed: "無法連線 GitHub：{detail}",
    invalid_proxy: "代理位址無效，範例：http://127.0.0.1:7890、socks5://127.0.0.1:7891",
  },
};
// What the button does in each service state (no task running).
const COMMAND = {
  not_installed: ["install", "install_service"],
  inactive: ["start", "start_service"],
  unreachable: ["restart", "restart_service"],
  running: ["open", "open_dashboard"],
};
const TASKS = ["installing", "starting", "stopping", "restarting"];

const $ = (id) => document.getElementById(id);
let command = null, open = false, text = TEXT.en, lastLog = "";
// Settings view: the proxy field is filled once, then owned by the user;
// results are kept as keys so a language switch re-renders them.
let proxyLoaded = false, languagePending = false, testing = false;
let languageResult = null, proxyResult = null;

function toggleText() {
  $("toggle").textContent = open ? text.hide + " ▾" : text.show + " ▴";
}

function render(state) {
  text = TEXT[state.language] || TEXT.en;
  document.documentElement.lang = state.language === "en" ? "en" : state.language === "zhtw" ? "zh-TW" : "zh-CN";

  // Only short, fixed phrases here; every detail goes to the output panel.
  const task = TASKS.includes(state.task) ? state.task : null;
  const action = task ? null : COMMAND[state.state];
  let status = text[task || state.state] || text.detecting;
  if (!task && state.failed) status = text["failed_" + state.failed] || status;
  else if (!task && state.state === "running" && state.version) status += " · " + state.version;
  $("status").textContent = status;

  command = action ? action[1] : null;
  const button = $("primary");
  $("label").textContent = action ? text[action[0]] : task ? text["busy_" + task] : text.wait;
  button.classList.toggle("loading", !action);
  button.disabled = !action;

  const log = $("log"), joined = state.log.join("\n");
  log.dataset.empty = text.empty;
  log.setAttribute("aria-label", text.output);
  if (joined !== lastLog) {
    const atBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 24;
    log.textContent = joined;
    lastLog = joined;
    if (atBottom) log.scrollTop = log.scrollHeight;
  }
  toggleText();
  renderSettings(state);
}

const format = (template, values) => template.replace(/\{(\w+)\}/g, (_, key) => values[key] ?? "");

function showResult(element, result) {
  element.className = "result" + (result ? " " + result.kind : "");
  element.textContent = result ? format(text[result.key] || result.key, result) : "";
  element.title = element.textContent;
}

function renderSettings(state) {
  for (const [id, label] of [["settings-open", text.settings], ["settings-back", text.back]]) {
    $(id).title = label;
    $(id).setAttribute("aria-label", label);
  }
  $("settings-title").textContent = text.settings;
  $("language-label").textContent = text.language;
  $("language-hint").textContent = text.language_hint;
  $("proxy-label").textContent = text.proxy;
  $("proxy-hint").textContent = text.proxy_hint;
  $("proxy-test-label").textContent = testing ? text.testing : text.test;
  if (!languagePending && document.activeElement !== $("language")) $("language").value = state.language;
  if (!proxyLoaded) {
    $("proxy").value = state.proxy || "";
    proxyLoaded = true;
  }
  showResult($("language-result"), languageResult);
  showResult($("proxy-result"), proxyResult);
}

function setView(settings) {
  document.body.classList.toggle("settings-open", settings);
  $("settings").hidden = !settings;
  (settings ? $("settings-back") : $("settings-open")).focus();
}

// Errors come back as a message; `invalid_proxy` is a fixed refusal.
const failure = (error) => String(error && error.message ? error.message : error);

async function applyProxy() {
  try {
    const proxy = await invoke("set_install_proxy", { proxy: $("proxy").value });
    $("proxy").value = proxy || "";
    proxyResult = proxy ? { kind: "ok", key: "proxy_set" } : null;
    return true;
  } catch (error) {
    const detail = failure(error);
    proxyResult = detail === "invalid_proxy" ? { kind: "error", key: "invalid_proxy" } : { kind: "error", key: detail };
    return false;
  } finally {
    showResult($("proxy-result"), proxyResult);
  }
}

async function refresh() {
  try { render(await invoke("local_state")); }
  catch (error) { console.error(error); }
}

$("primary").addEventListener("click", async () => {
  if (!command) return;
  $("primary").disabled = true;
  // A refusal is written to the output panel by the app itself.
  try { await invoke(command); }
  catch (error) { console.error(error); }
  refresh();
});

$("toggle").addEventListener("click", () => {
  open = !open;
  $("output").classList.toggle("open", open);
  $("toggle").setAttribute("aria-expanded", String(open));
  toggleText();
  if (open) $("log").scrollTop = $("log").scrollHeight;
});

$("settings-open").addEventListener("click", () => setView(true));
$("settings-back").addEventListener("click", () => setView(false));
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !$("settings").hidden) setView(false);
});

$("language").addEventListener("change", async () => {
  const select = $("language");
  languagePending = true;
  select.disabled = true;
  languageResult = null;
  try { await invoke("set_interface_language", { language: select.value }); }
  catch (error) { languageResult = { kind: "error", key: "language_failed", detail: failure(error) }; }
  languagePending = false;
  select.disabled = false;
  refresh();
});

$("proxy").addEventListener("change", applyProxy);
$("proxy").addEventListener("keydown", (event) => {
  if (event.key === "Enter") applyProxy();
});

$("proxy-test").addEventListener("click", async () => {
  if (testing || !(await applyProxy())) return;
  const button = $("proxy-test"), proxy = $("proxy").value;
  testing = true;
  button.disabled = true;
  button.classList.add("loading");
  $("proxy-test-label").textContent = text.testing;
  proxyResult = { kind: "pending", key: "testing" };
  showResult($("proxy-result"), proxyResult);
  try {
    const ms = await invoke("test_install_proxy", { proxy });
    proxyResult = { kind: "ok", key: proxy ? "proxy_ok" : "direct_ok", ms };
  } catch (error) {
    const detail = failure(error);
    proxyResult = detail === "invalid_proxy" ? { kind: "error", key: "invalid_proxy" } : { kind: "error", key: "proxy_failed", detail };
  }
  testing = false;
  button.disabled = false;
  button.classList.remove("loading");
  $("proxy-test-label").textContent = text.test;
  showResult($("proxy-result"), proxyResult);
});

refresh();
setInterval(refresh, 1000);
