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
  message: string;
}

export interface BinaryStartMarker {
  type: 'binary_start';
}

export interface ThreadSegment {
  begin: number;
  end: number;
}

export interface ThreadCoverage {
  streamId: string;
  threadName: string;
  segments: ThreadSegment[];
}

export interface PropertySegment {
  value: string;
  begin: number; // ms timestamp
  end: number; // ms timestamp
}

export interface PropertyTimelineData {
  propertyName: string;
  segments: PropertySegment[];
}