import React, { useState, useEffect } from 'react';
import { Checkbox, InlineFieldRow, SegmentSection } from '@grafana/ui';
import { SQLQuery, getTimeFilter } from '../types';

interface VariableQueryProps {
  query: SQLQuery | string;
  onChange: (query: SQLQuery, definition: string) => void;
}

export const VariableQueryEditor = ({ onChange, query: queryProp }: VariableQueryProps) => {
  // Normalize query on first render - convert old formats to new SQLQuery format
  const [query, setQuery] = useState<SQLQuery>(() => {
    // Case 1: Legacy string format (when no custom editor was registered)
    if (typeof queryProp === 'string') {
      return {
        query: queryProp,        // For Grafana's definition display
        queryText: queryProp,
        refId: 'A',
        timeFilter: true,
        autoLimit: false
      };
    }

    // Case 2: Modern format - just ensure defaults
    const q = queryProp || { queryText: '', refId: 'A' };
    return {
      ...q,
      query: q.queryText || q.query,  // Sync query field for Grafana
      timeFilter: getTimeFilter(q),
      autoLimit: false
    };
  });

  const [timeFilter, setTimeFilter] = useState(query.timeFilter ?? true);

  useEffect(() => {
    // Update query when timeFilter changes
    const updatedQuery = {
      ...query,
      timeFilter: timeFilter,
      query: query.queryText  // Keep query field in sync for Grafana's definition
    };
    setQuery(updatedQuery);
    // Provide a descriptive definition string that includes query text
    const definition = updatedQuery.queryText || '';
    onChange(updatedQuery, definition);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [timeFilter]);

  const saveQuery = () => {
    // Sync the query field for Grafana's automatic definition extraction
    const updatedQuery = {
      ...query,
      query: query.queryText  // Keep query field in sync for Grafana's definition
    };
    setQuery(updatedQuery);
    // Provide a descriptive definition string that includes query text
    const definition = updatedQuery.queryText || '';
    onChange(updatedQuery, definition);
  };

  const handleChange = (event: React.FormEvent<HTMLInputElement>) => {
    const updatedQuery = {
      ...query,
      [event.currentTarget.name]: event.currentTarget.value,
    };
    setQuery(updatedQuery);
  };

  return (
    <>
      <div className="gf-form">
        <span className="gf-form-label width-10">Query</span>
        <input
          name="queryText"
          className="gf-form-input"
          onBlur={saveQuery}
          onChange={handleChange}
          value={query.queryText || ''}
        />
      </div>

      <InlineFieldRow style={{ flexFlow: 'row', alignItems: 'center' }}>
        <SegmentSection label="Time Filter">
          <Checkbox value={timeFilter} onChange={() => { setTimeFilter(!timeFilter); }} />
        </SegmentSection>
      </InlineFieldRow>
    </>
  );
};
