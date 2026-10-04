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
    show: "Show output", hide: "Hide output",
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
    show: "显示输出", hide: "收起输出",
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
    show: "顯示輸出", hide: "收起輸出",
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

function render(state) {
  text = TEXT[state.language] || TEXT.en;
  document.documentElement.lang = state.language === "en" ? "en" : state.language === "zhtw" ? "zh-TW" : "zh-CN";

  const task = TASKS.includes(state.task) ? state.task : null;
  const action = task ? null : COMMAND[state.state];
  let status = text[task || state.state] || text.detecting;
  if (!task && state.state === "running" && state.version) status += " · " + state.version;
  $("status").textContent = status;

  command = action ? action[1] : null;
  const button = $("primary");
  $("label").textContent = action ? text[action[0]] : task ? text["busy_" + task] : text.wait;
  button.classList.toggle("loading", !action);
  button.disabled = !action;

  const note = $("note");
  note.textContent = state.last_error || (state.state === "unreachable" && state.detail) || "";
  note.classList.toggle("error", Boolean(state.last_error));

  const log = $("log"), joined = state.log.join("\n");
  $("output").classList.toggle("has-log", joined !== "");
  if (joined !== lastLog) {
    const atBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 24;
    log.textContent = joined;
    lastLog = joined;
    if (atBottom) log.scrollTop = log.scrollHeight;
  }
  $("toggle").textContent = (open ? text.hide + " ▾" : text.show + " ▴");
}

async function refresh() {
  try { render(await invoke("local_state")); }
  catch (error) { $("note").textContent = String(error); }
}

$("primary").addEventListener("click", async () => {
  if (!command) return;
  $("primary").disabled = true;
  try { await invoke(command); }
  catch (error) { $("note").textContent = String(error); }
  refresh();
});

$("toggle").addEventListener("click", () => {
  open = !open;
  $("output").classList.toggle("open", open);
  $("toggle").setAttribute("aria-expanded", String(open));
  $("toggle").textContent = open ? text.hide + " ▾" : text.show + " ▴";
  if (open) $("log").scrollTop = $("log").scrollHeight;
});

refresh();
setInterval(refresh, 1000);
