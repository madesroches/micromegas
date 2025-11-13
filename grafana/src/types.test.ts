import { migrateQuery, SQLQuery, QueryContext } from './types';

describe('migrateQuery', () => {
  describe('Panel context', () => {
    it('should migrate v1 query with queryText to v2 with query field', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT time, process_id, level, target, msg FROM log_entries WHERE level <= 4 ORDER BY time DESC LIMIT 100',
      };

      const result = migrateQuery(v1Query, QueryContext.Panel);

      expect(result.query).toBe('SELECT time, process_id, level, target, msg FROM log_entries WHERE level <= 4 ORDER BY time DESC LIMIT 100');
      expect(result.queryText).toBeUndefined();
      expect(result.version).toBe(2);
    });

    it('should set default format to table when undefined', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT process_id, exe, computer FROM processes ORDER BY start_time DESC',
      };

      const result = migrateQuery(v1Query, QueryContext.Panel);

      expect(result.format).toBe('table');
    });

    it('should preserve explicit format value', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT time, level, msg FROM log_entries ORDER BY time DESC',
        format: 'logs',
      };

      const result = migrateQuery(v1Query, QueryContext.Panel);

      expect(result.format).toBe('logs');
    });

    it('should set default timeFilter to true when undefined', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT time, level, msg FROM log_entries LIMIT 100',
      };

      const result = migrateQuery(v1Query, QueryContext.Panel);

      expect(result.timeFilter).toBe(true);
    });

    it('should preserve explicit timeFilter false value', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT process_id, exe FROM processes',
        timeFilter: false,
      };

      const result = migrateQuery(v1Query, QueryContext.Panel);

      expect(result.timeFilter).toBe(false);
    });

    it('should set default autoLimit to true when undefined', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT time, span_id, name FROM spans ORDER BY time DESC',
      };

      const result = migrateQuery(v1Query, QueryContext.Panel);

      expect(result.autoLimit).toBe(true);
    });

    it('should preserve explicit autoLimit false value', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT COUNT(*) as count FROM log_entries',
        autoLimit: false,
      };

      const result = migrateQuery(v1Query, QueryContext.Panel);

      expect(result.autoLimit).toBe(false);
    });

    it('should handle query field taking precedence over queryText', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        query: 'SELECT name, value FROM metrics ORDER BY time DESC',
        queryText: 'SELECT time, level FROM log_entries',
      };

      const result = migrateQuery(v1Query, QueryContext.Panel);

      expect(result.query).toBe('SELECT name, value FROM metrics ORDER BY time DESC');
      expect(result.queryText).toBeUndefined();
      expect(result.version).toBe(2);
    });

    it('should handle query with only query field (no queryText)', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        query: 'SELECT time, value FROM metrics WHERE name = \'cpu_usage\'',
      };

      const result = migrateQuery(v1Query, QueryContext.Panel);

      expect(result.query).toBe('SELECT time, value FROM metrics WHERE name = \'cpu_usage\'');
      expect(result.queryText).toBeUndefined();
      expect(result.version).toBe(2);
    });

    it('should handle query with neither query nor queryText', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
      };

      const result = migrateQuery(v1Query, QueryContext.Panel);

      expect(result.query).toBe('');
      expect(result.queryText).toBeUndefined();
      expect(result.version).toBe(2);
    });
  });

  describe('Variable context', () => {
    it('should force autoLimit to false for variables', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT DISTINCT computer FROM processes',
      };

      const result = migrateQuery(v1Query, QueryContext.Variable);

      expect(result.autoLimit).toBe(false);
    });

    it('should override explicit autoLimit true to false for variables', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT DISTINCT exe FROM processes',
        autoLimit: true,
      };

      const result = migrateQuery(v1Query, QueryContext.Variable);

      expect(result.autoLimit).toBe(false);
    });

    it('should set default format to table for variables', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT DISTINCT level FROM log_entries',
      };

      const result = migrateQuery(v1Query, QueryContext.Variable);

      expect(result.format).toBe('table');
    });

    it('should set default timeFilter to true for variables', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT DISTINCT target FROM log_entries',
      };

      const result = migrateQuery(v1Query, QueryContext.Variable);

      expect(result.timeFilter).toBe(true);
    });
  });

  describe('Edge cases', () => {
    it('should handle null query', () => {
      const result = migrateQuery(null, QueryContext.Panel);

      expect(result.query).toBe('');
      expect(result.format).toBe('table');
      expect(result.timeFilter).toBe(true);
      expect(result.autoLimit).toBe(true);
      expect(result.version).toBe(2);
    });

    it('should handle undefined query', () => {
      const result = migrateQuery(undefined, QueryContext.Panel);

      expect(result.query).toBe('');
      expect(result.format).toBe('table');
      expect(result.timeFilter).toBe(true);
      expect(result.autoLimit).toBe(true);
      expect(result.version).toBe(2);
    });

    it('should handle empty query object', () => {
      const result = migrateQuery({} as SQLQuery, QueryContext.Panel);

      expect(result.query).toBe('');
      expect(result.format).toBe('table');
      expect(result.timeFilter).toBe(true);
      expect(result.autoLimit).toBe(true);
      expect(result.version).toBe(2);
    });

    it('should handle legacy string query', () => {
      const result = migrateQuery('SELECT time, level, msg FROM log_entries ORDER BY time DESC LIMIT 100', QueryContext.Panel);

      expect(result.query).toBe('SELECT time, level, msg FROM log_entries ORDER BY time DESC LIMIT 100');
      expect(result.queryText).toBeUndefined();
      expect(result.format).toBe('table');
      expect(result.timeFilter).toBe(true);
      expect(result.autoLimit).toBe(true);
      expect(result.version).toBe(2);
    });

    it('should handle legacy string query in variable context', () => {
      const result = migrateQuery('SELECT DISTINCT computer FROM processes', QueryContext.Variable);

      expect(result.query).toBe('SELECT DISTINCT computer FROM processes');
      expect(result.queryText).toBeUndefined();
      expect(result.autoLimit).toBe(false);
      expect(result.version).toBe(2);
    });

    it('should handle query with explicit version 1', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT process_id, exe FROM processes',
        version: 1,
      };

      const result = migrateQuery(v1Query, QueryContext.Panel);

      expect(result.query).toBe('SELECT process_id, exe FROM processes');
      expect(result.queryText).toBeUndefined();
      expect(result.version).toBe(2);
    });

    it('should treat invalid version numbers as v2 (forward compatibility)', () => {
      const futureQuery: SQLQuery = {
        refId: 'A',
        query: 'SELECT time, span_id FROM spans',
        format: 'table',
        timeFilter: true,
        autoLimit: true,
        version: 99,
      };

      const result = migrateQuery(futureQuery, QueryContext.Panel);

      expect(result).toEqual(futureQuery);
    });

    it('should migrate negative version numbers as v1', () => {
      const invalidQuery: SQLQuery = {
        refId: 'A',
        query: 'SELECT time, level FROM log_entries',
        format: 'table',
        timeFilter: true,
        autoLimit: true,
        version: -1,
      };

      const result = migrateQuery(invalidQuery, QueryContext.Panel);

      // Negative versions are treated as v1 and migrated
      expect(result.version).toBe(2);
      expect(result.query).toBe('SELECT time, level FROM log_entries');
      expect(result.queryText).toBeUndefined();
    });
  });

  describe('Idempotency', () => {
    it('should not change v2 queries when migrated', () => {
      const v2Query: SQLQuery = {
        refId: 'A',
        query: 'SELECT time, level, msg FROM log_entries ORDER BY time DESC',
        format: 'table',
        timeFilter: true,
        autoLimit: false,
        version: 2,
      };

      const result = migrateQuery(v2Query, QueryContext.Panel);

      expect(result).toEqual(v2Query);
    });

    it('should be idempotent - migrating twice gives same result', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT process_id, exe FROM processes',
      };

      const firstMigration = migrateQuery(v1Query, QueryContext.Panel);
      const secondMigration = migrateQuery(firstMigration, QueryContext.Panel);

      expect(secondMigration).toEqual(firstMigration);
    });

    it('should adjust autoLimit for v2 variable queries if needed', () => {
      const v2Query: SQLQuery = {
        refId: 'A',
        query: 'SELECT DISTINCT computer FROM processes',
        format: 'table',
        timeFilter: true,
        autoLimit: true,
        version: 2,
      };

      const result = migrateQuery(v2Query, QueryContext.Variable);

      expect(result.autoLimit).toBe(false);
    });
  });

  describe('Immutability', () => {
    it('should not mutate the input query', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT time, span_id FROM spans',
      };

      const original = { ...v1Query };
      migrateQuery(v1Query, QueryContext.Panel);

      expect(v1Query).toEqual(original);
    });
  });

  describe('Complex scenarios', () => {
    it('should handle query with mixed v1 fields', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT time, level, msg FROM log_entries WHERE level <= 3',
        format: 'logs',
        timeFilter: false,
        autoLimit: false,
        table: 'log_entries',
        rawEditor: true,
      };

      const result = migrateQuery(v1Query, QueryContext.Panel);

      expect(result.query).toBe('SELECT time, level, msg FROM log_entries WHERE level <= 3');
      expect(result.queryText).toBeUndefined();
      expect(result.format).toBe('logs');
      expect(result.timeFilter).toBe(false);
      expect(result.autoLimit).toBe(false);
      expect(result.table).toBe('log_entries');
      expect(result.rawEditor).toBe(true);
      expect(result.version).toBe(2);
    });

    it('should preserve other query fields during migration', () => {
      const v1Query: SQLQuery = {
        refId: 'A',
        queryText: 'SELECT time, level, target, msg FROM log_entries',
        table: 'log_entries',
        columns: ['time', 'level', 'target', 'msg'],
        wheres: ['level <= 3'],
        orderBy: 'time DESC',
        groupBy: 'process_id',
        limit: '100',
        rawEditor: false,
      };

      const result = migrateQuery(v1Query, QueryContext.Panel);

      expect(result.table).toBe('log_entries');
      expect(result.columns).toEqual(['time', 'level', 'target', 'msg']);
      expect(result.wheres).toEqual(['level <= 3']);
      expect(result.orderBy).toBe('time DESC');
      expect(result.groupBy).toBe('process_id');
      expect(result.limit).toBe('100');
      expect(result.rawEditor).toBe(false);
    });
  });
});
