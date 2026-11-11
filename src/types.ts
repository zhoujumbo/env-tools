export type PageId =
  | "hosts"
  | "env"
  | "node"
  | "python"
  | "java"
  | "golang"
  | "logs"
  | "settings";

export interface AppPaths {
  root_dir: string;
  from_envt_path: boolean;
  envt_path: string | null;
  config_dir: string;
  data_dir: string;
  log_dir: string;
}

export type ProxyMode = "none" | "system" | "manual";

export interface Prefs {
  flush_dns_after_hosts: boolean;
  backup_keep: number;
  theme: "dark" | "light" | "system" | string;
  node_pkg_override: boolean;
  node_pkg_root: string;
  proxy_mode: ProxyMode | string;
  proxy_host: string;
  proxy_port: number;
  proxy_user: string;
  proxy_pass: string;
  minimize_to_tray: boolean;
}

export interface HostsEntry {
  id: string;
  title: string;
  on: boolean;
  kind: string;
}

export interface TrashItem {
  id: string;
  title: string;
  content: string;
  deleted_at: string;
}

export interface EnvVar {
  name: string;
  value: string;
}

export interface EnvChange {
  name: string;
  action: "set" | "delete";
  value?: string | null;
}

export interface EnvSnapshot {
  user: EnvVar[];
  system: EnvVar[];
}

export interface OpRecord {
  id: string;
  ts: string;
  actor: string;
  kind: string;
  cmdline: string | null;
  cwd: string | null;
  exit_code: number | null;
  stdout_path: string | null;
  stderr_path: string | null;
  before_json: string | null;
  after_json: string | null;
  undo_json: string | null;
  ok: boolean;
}

export interface VersionMeta {
  version: string;
  npm: string | null;
  pip: string | null;
  pnpm: string | null;
}

export interface RuntimeStatus {
  kind: string;
  root: string;
  current: string | null;
  current_path: string | null;
  versions: string[];
  version_meta: VersionMeta[];
  path_ok: boolean;
  path_entries: string[];
  cmd_which: string | null;
  store_alias: boolean;
  pnpm_cmd_ok: boolean;
  pnpm_needs_fix: boolean;
  pnpm_which: string | null;
}

export interface RemoteNodeVersion {
  version: string;
  lts: string | null;
}

export interface RemoteNodeList {
  versions: RemoteNodeVersion[];
  from_cache: boolean;
  stale: boolean;
  fetched_at: number | null;
  ttl_secs: number;
}

export interface RuntimeProgress {
  runtime: string;
  op: string;
  phase: string;
  version: string;
  percent: number | null;
  bytes: number | null;
  total: number | null;
  message: string;
}

export interface ShellCompatStatus {
  ps5_path: string;
  ps5_injected: boolean;
  ps5_exists: boolean;
  ps7_path: string;
  ps7_injected: boolean;
  ps7_exists: boolean;
  execution_policy: string;
  cmd_init_path: string;
  cmd_init_exists: boolean;
  cmd_autorun: string | null;
  cmd_autorun_points_to_us: boolean;
}

export interface BackupInfo {
  name: string;
  path: string;
  size: number;
  created_at: string;
}

export interface BaselineManifest {
  captured_at: string;
  hosts_path: string;
  autorun: string | null;
  ps5_existed: boolean;
  ps7_existed: boolean;
}

export interface ApplyOutcome {
  previous_content: string;
  new_content: string;
  unchanged: boolean;
}

export interface AppMemory {
  working_set: number;
  private: number;
  processes: number;
  rust_working_set: number;
  webview_working_set: number;
}

export interface RuntimeLogFile {
  name: string;
  path: string;
  size: number;
  modified: number | null;
}

export interface NodePkgLayout {
  enabled: boolean;
  root: string;
  custom: boolean;
  global_dir: string;
  cache_dir: string;
  npm_prefix_default: string;
  npm_cache_default: string;
  pnpm_home_default: string;
  pnpm_store_default: string;
  pnpm_cache_default: string;
  global_exists: boolean;
  cache_exists: boolean;
}

export interface PnpmEnv {
  installed: boolean;
  cmd_ok: boolean;
  needs_fix: boolean;
  shim: string | null;
  which: string | null;
  version: string | null;
  detail: string;
}

export interface PrefsSaveResult {
  pkg_changed: boolean;
  detail: string;
  npm: NpmUserConfig | null;
  pnpm: PnpmEnv | null;
}

export interface NpmUserConfig {
  available: boolean;
  version: string | null;
  prefix: string;
  cache: string;
  registry: string;
  prefix_desired: string;
  cache_desired: string;
  prefix_ok: boolean;
  cache_ok: boolean;
  custom: boolean;
  detail: string;
}

export interface ToolCache {
  id: string;
  tool: string;
  title: string;
  path: string;
  exists: boolean;
  size: number;
}

export interface JavaEnvReport {
  current: string | null;
  current_home: string | null;
  current_bin: string | null;
  user_java_home: string | null;
  machine_java_home: string | null;
  user_path_hits: string[];
  machine_path_hits: string[];
  foreign_install_hint: string | null;
  needs_claim: boolean;
}
