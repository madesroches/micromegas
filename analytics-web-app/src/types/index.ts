export interface ProcessInfo {
  process_id: string;
  exe: string;
  start_time: string;
  last_update_time: string;
  computer: string;
  username: string;
  cpu_brand: string;
  distro: string;
  properties: Record<string, string>;
}


export interface GenerateTraceRequest {
  time_range?: {
    begin: string;
    end: string;
  };
  include_async_spans: boolean;
  include_thread_spans: boolean;
}

export interface ProgressUpdate {
  type: 'progress';
  percentage: number;
  message: string;
}

export interface BinaryStartMarker {
  type: 'binary_start';
}

export interface HealthCheck {
  status: string;
  timestamp: string;
  flightsql_connected: boolean;
}

export interface ProcessStatistics {
  process_id: string;
  log_entries: number;
  measures: number;
  trace_events: number;
  thread_count: number;
}

export interface LogEntry {
  time: string;
  level: string;
  target: string;
  msg: string;
}