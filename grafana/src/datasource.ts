import {DataQueryResponse, MetricFindValue, DataSourceInstanceSettings, CoreApp, ScopedVars, VariableWithMultiSupport} from '@grafana/data'
import {frameToMetricFindValue, DataSourceWithBackend, getTemplateSrv} from '@grafana/runtime'
import {SQLQuery, QueryFormat, FlightSQLDataSourceOptions, DEFAULT_QUERY, getTimeFilter} from './types'
import { lastValueFrom } from 'rxjs';


export class FlightSQLDataSource extends DataSourceWithBackend<SQLQuery, FlightSQLDataSourceOptions> {
  constructor(instanceSettings: DataSourceInstanceSettings<FlightSQLDataSourceOptions>) {
    super(instanceSettings)
  }

  // Called by Grafana to populate dashboard variable dropdowns (legacy name from metrics datasources)
  async metricFindQuery(query: SQLQuery | string, options?: any): Promise<MetricFindValue[]> {
      // Handle both string (legacy) and SQLQuery object inputs
      const queryText = typeof query === 'string' ? query : query.queryText;
      const queryObj = typeof query === 'string' ? {} : query;

      const target: SQLQuery = {
        refId: 'metricFindQuery',
        queryText,
        rawEditor: true,
        format: QueryFormat.Table,
        timeFilter: getTimeFilter(queryObj as SQLQuery),
        autoLimit: false  // Always false for variable queries (no limit in variable context)
      };
      return lastValueFrom(
        super.query({
          ...(options ?? {}), // includes 'range'
          targets: [target],
        })
      ).then(this.toMetricFindValue);
  }

  toMetricFindValue(rsp: DataQueryResponse): MetricFindValue[] {
      const data = rsp.data ?? [];
      // Create MetricFindValue object for all frames
      const values = data.map((d) => frameToMetricFindValue(d)).flat();
      // Filter out duplicate elements
      return values.filter((elm, idx, self) => idx === self.findIndex((t) => t.text === elm.text));
                             
  }

  getDefaultQuery(_: CoreApp): Partial<SQLQuery> {
    return DEFAULT_QUERY
  }

  quoteLiteral(value: string) {
    return "'" + value.replace(/'/g, "''") + "'"
  }

  interpolateVariable = (value: string | string[] | number, variable: VariableWithMultiSupport) => {
    if (typeof value === 'string') {
      if (variable?.multi || variable?.includeAll) {
        return this.quoteLiteral(value)
      } else {
        return String(value).replace(/'/g, "''")
      }
    }

    if (typeof value === 'number') {
      return value
    }

    if (Array.isArray(value)) {
      const quotedValues = value.map((v) => this.quoteLiteral(v))
      return quotedValues.join(',')
    }

    return value
  }

  applyTemplateVariables(query: SQLQuery, scopedVars: ScopedVars): SQLQuery {
    const interpolatedQuery: SQLQuery = {
      ...query,
      queryText: getTemplateSrv().replace(query.queryText, scopedVars, this.interpolateVariable),
    }
    return interpolatedQuery
  }

  getSQLInfo(): Promise<any> {
    return this.getResource('/flightsql/sql-info')
  }

  getTables(): Promise<any> {
    return this.getResource('/flightsql/tables')
  }

  getColumns(table: string): Promise<any> {
    return this.getResource(`/flightsql/columns?table=${table}`)
  }

  getMacros(): Promise<any> {
    return this.getResource('/plugin/macros')
  }
}
