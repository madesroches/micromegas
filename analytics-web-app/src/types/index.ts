export interface ProcessInfo {
  process_id: string;
  exe: string;
  begin: string;
  end: string;
  computer: string;
  username: string;
  cpu_brand: string;
  distro: string;
  properties: Record<string, string>;
}

export interface SpanCounts {
  thread_spans: number;
  async_spans: number;
  total: number;
}

export interface TraceMetadata {
  process_id: string;
  estimated_size_bytes?: number;
  span_counts: SpanCounts;
  generation_time_estimate: number; // seconds
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