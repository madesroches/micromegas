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