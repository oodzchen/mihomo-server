import { savedLanguage, t } from "./i18n";
import type { EventMessage } from "./types";

/** WebSocket session state; also the i18n key used to display it. */
export type Connection =
  | "connecting"
  | "connected"
  | "reconnecting"
  | "unauthorized"
  | "badData";

export type Perform = <T>(
  name: string,
  fields?: Record<string, unknown>,
  options?: { notify?: boolean },
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
