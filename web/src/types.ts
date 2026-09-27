export interface CoreStatus {
  phase: string;
  pid?: number;
  version?: string;
  error?: string;
  generation: number;
  config_revision?: string;
  active_profile?: string;
  selection_pending: string[];
  selection_error?: string;
}
export interface Profile {
  uid: string;
  file?: string;
  name?: string;
  type?: string;
  desc?: string;
  url?: string;
  option?: {
    merge?: string;
    script?: string;
    rules?: string;
    proxies?: string;
    groups?: string;
    user_agent?: string;
    timeout_seconds?: number;
    update_interval?: number;
    allow_auto_update?: boolean;
    self_proxy?: boolean;
    with_proxy?: boolean;
    danger_accept_invalid_certs?: boolean;
  };
  extra?: { upload: number; download: number; total: number; expire: number };
  selected?: { name?: string; now?: string }[];
}
export interface Profiles {
  current?: string;
  items?: Profile[];
}
export interface CoreLog {
  stream: string;
  message: string;
}
export interface Proxy {
  name: string;
  type: string;
  all?: string[];
  now?: string;
  fixed?: string;
}
export interface Proxies {
  proxies: Record<string, Proxy>;
}
export type EventMessage = {
  type: string;
  data?: unknown;
  status?: CoreStatus;
  profiles?: Profiles;
  logs?: CoreLog[];
  code?: string;
  message?: string;
};
