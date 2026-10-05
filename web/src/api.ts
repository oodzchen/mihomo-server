import { savedLanguage, t } from "./i18n";
import type { ToastOperation } from "./toast";
import type { EventMessage } from "./types";

/** WebSocket session state; also the i18n key used to display it. */
export type Connection =
  | "connecting"
  | "connected"
  | "reconnecting"
  | "unauthorized"
  | "badData";

/** A supplied toast owns the specific result; reconcile defers errors until caller readback. */
export type Perform = <T>(
  name: string,
  fields?: Record<string, unknown>,
  options?: { notify?: boolean; toast?: ToastOperation; reconcile?: boolean },
) => Promise<T | undefined>;

export class ApiError extends Error {
  constructor(
    message: string,
    public status: number,
  ) {
    super(message);
  }
}
export async function command<T>(
  token: string,
  command: string,
  fields: Record<string, unknown> = {},
  signal?: AbortSignal,
): Promise<T> {
  const lang = savedLanguage();
  const response = await fetch("/api/commands", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${token}`,
      "Accept-Language": lang,
    },
    body: JSON.stringify({ command, ...fields }),
    signal,
    cache: "no-store",
  });
  const body = await response.json();
  if (!response.ok)
    throw new ApiError(
      body.error?.message || t(lang, "requestFailed", { status: response.status }),
      response.status,
    );
  return body as T;
}

/** The built files a page loads; Vite names them by content hash. */
function buildSignature(page: Document) {
  return [...page.querySelectorAll('script[type="module"][src], link[rel="stylesheet"][href]')]
    .map((element) => element.getAttribute("src") ?? element.getAttribute("href"))
    .join("\n");
}

/** Whether the service now serves another build of this page (it was upgraded). */
export async function servedBuildChanged(signal?: AbortSignal) {
  const response = await fetch("/", { headers: { Accept: "text/html" }, cache: "no-store", signal });
  if (!response.ok) return false;
  const served = buildSignature(new DOMParser().parseFromString(await response.text(), "text/html"));
  return served !== "" && served !== buildSignature(document);
}

const BUILD_RELOAD_KEY = "mihomo.buildReload";

/** Whether this page load is `reloadToServedBuild`'s, read once at startup. */
export const loadedServedBuild = (() => {
  try {
    const reloaded = window.sessionStorage.getItem(BUILD_RELOAD_KEY) !== null;
    window.sessionStorage.removeItem(BUILD_RELOAD_KEY);
    return reloaded;
  } catch {
    return false;
  }
})();

export function reloadToServedBuild() {
  try { window.sessionStorage.setItem(BUILD_RELOAD_KEY, "1"); }
  catch { /* Private browser storage can be unavailable. */ }
  window.location.reload();
}

/** One owned socket and reconnect timer; teardown cancels both, including pending auth. */
export function subscribe(
  token: string,
  path: string,
  receive: (message: EventMessage) => void,
  connection: (state: Connection) => void,
): () => void {
  let stopped = false,
    socket: WebSocket | undefined,
    timer: ReturnType<typeof setTimeout> | undefined,
    attempts = 0;
  function open() {
    if (stopped) return;
    connection(attempts ? "reconnecting" : "connecting");
    const current = new WebSocket(
      `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}${path}`,
    );
    socket = current;
    current.onopen = () => {
      if (!stopped)
        current.send(JSON.stringify({ type: "authenticate", token }));
    };
    current.onmessage = ({ data }) => {
      if (stopped || socket !== current) return;
      try {
        const message = JSON.parse(data) as EventMessage;
        if (message.type === "ready") {
          attempts = 0;
          connection("connected");
        }
        if (message.code === "unauthorized") {
          stopped = true;
          connection("unauthorized");
          current.close();
        }
        receive(message);
      } catch {
        connection("badData");
        current.close();
      }
    };
    current.onclose = (event) => {
      if (stopped || socket !== current) return;
      if (event.code === 1008) {
        connection("unauthorized");
        return;
      }
      connection("reconnecting");
      timer = setTimeout(open, Math.min(1000 * 2 ** attempts++, 10000));
    };
    current.onerror = () => current.close();
  }
  open();
  return () => {
    stopped = true;
    clearTimeout(timer);
    socket?.close();
  };
}
