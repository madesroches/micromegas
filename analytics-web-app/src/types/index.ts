/** Generic row from SQL query results */
export type SqlRow = Record<string, string | number | boolean | null>;

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

export interface SqlQueryRequest {
  sql: string;
  params?: Record<string, string>;
  begin?: string;
  end?: string;
}

export interface SqlQueryResponse {
  columns: string[];
  rows: (string | number | boolean | null)[][];
}

export interface SqlQueryError {
  error: string;
  details?: string;
}