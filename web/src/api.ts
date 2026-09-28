import { savedLanguage } from "./i18n";
import type { EventMessage } from "./types";

export type Perform = <T>(
  name: string,
  fields?: Record<string, unknown>,
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
      body.error?.message ||
        (lang === "en" ? `Request failed (${response.status})` : `请求失败 (${response.status})`),
      response.status,
    );
  return body as T;
}

/** One owned socket and reconnect timer; teardown cancels both, including pending auth. */
export function subscribe(
  token: string,
  path: string,
  receive: (message: EventMessage) => void,
  connection: (state: string) => void,
): () => void {
  let stopped = false,
    socket: WebSocket | undefined,
    timer: ReturnType<typeof setTimeout> | undefined,
    attempts = 0;
  function open() {
    if (stopped) return;
    connection(attempts ? "重连中" : "连接中");
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
          connection("已连接");
        }
        if (message.code === "unauthorized") {
          stopped = true;
          connection("认证失败");
          current.close();
        }
        receive(message);
      } catch {
        connection("数据错误");
        current.close();
      }
    };
    current.onclose = (event) => {
      if (stopped || socket !== current) return;
      if (event.code === 1008) {
        connection("认证失败");
        return;
      }
      connection("重连中");
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
