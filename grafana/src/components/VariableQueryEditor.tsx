import React, { useState, useEffect } from 'react';
import { Checkbox, InlineFieldRow, SegmentSection } from '@grafana/ui';
import { SQLQuery, migrateQuery } from '../types';

interface VariableQueryProps {
  query: SQLQuery | string;
  onChange: (query: SQLQuery, definition: string) => void;
}

export const VariableQueryEditor = ({ onChange, query: queryProp }: VariableQueryProps) => {
  // Migrate query on first render using centralized migration function
  const [query, setQuery] = useState<SQLQuery>(() => {
    return migrateQuery(queryProp, 'variable');
  });

  const [timeFilter, setTimeFilter] = useState(query.timeFilter ?? true);

  useEffect(() => {
    // Update query when timeFilter changes
    const updatedQuery = {
      ...query,
      timeFilter: timeFilter,
    };
    setQuery(updatedQuery);
    // Provide the query text as definition for Grafana
    const definition = updatedQuery.query || '';
    onChange(updatedQuery, definition);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [timeFilter]);

  const saveQuery = () => {
    // Provide the query text as definition for Grafana
    const definition = query.query || '';
    onChange(query, definition);
  };

  const handleChange = (event: React.FormEvent<HTMLInputElement>) => {
    const updatedQuery = {
      ...query,
      query: event.currentTarget.value,  // Update the query field (v2)
    };
    setQuery(updatedQuery);
  };

  return (
    <>
      <div className="gf-form">
        <span className="gf-form-label width-10">Query</span>
        <input
          name="query"
          className="gf-form-input"
          onBlur={saveQuery}
          onChange={handleChange}
          value={query.query || ''}
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
