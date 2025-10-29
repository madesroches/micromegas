// Shared types for Micromegas monorepo

export interface ProcessInfo {
  process_id: string;
  parent_process_id?: string;
  exe_path?: string;
  command_line?: string;
  working_directory?: string;
  username?: string;
  machine_name?: string;
  start_time?: number;
}

export interface StreamInfo {
  stream_id: string;
  process_id: string;
  stream_type: string;
  name?: string;
  tags?: Record<string, string>;
}

export interface LogEntry {
  timestamp: number;
  level: string;
  message: string;
  file?: string;
  line?: number;
  thread_id?: string;
}

export interface MetricPoint {
  timestamp: number;
  name: string;
  value: number;
  unit?: string;
  tags?: Record<string, string>;
}

export interface SpanEvent {
  timestamp: number;
  span_id: string;
  trace_id?: string;
  parent_span_id?: string;
  operation_name: string;
  duration?: number;
  tags?: Record<string, string>;
}

export interface AuthConfig {
  auth_type: 'none' | 'token' | 'username_password' | 'oidc';
  token?: string;
  username?: string;
  password?: string;
  oidc_issuer?: string;
  oidc_client_id?: string;
}

export interface ConnectionConfig {
  host: string;
  port?: number;
  secure?: boolean;
  auth?: AuthConfig;
  metadata?: Record<string, any>;
}
