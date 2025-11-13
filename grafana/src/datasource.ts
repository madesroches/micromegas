import {DataQueryResponse, MetricFindValue, DataSourceInstanceSettings, CoreApp, ScopedVars, VariableWithMultiSupport} from '@grafana/data'
import {frameToMetricFindValue, DataSourceWithBackend, getTemplateSrv} from '@grafana/runtime'
import {SQLQuery, QueryFormat, FlightSQLDataSourceOptions, DEFAULT_QUERY, migrateQuery, QueryContext} from './types'
import { lastValueFrom } from 'rxjs';


export class FlightSQLDataSource extends DataSourceWithBackend<SQLQuery, FlightSQLDataSourceOptions> {
  constructor(instanceSettings: DataSourceInstanceSettings<FlightSQLDataSourceOptions>) {
    super(instanceSettings)
  }

  // Called by Grafana to populate dashboard variable dropdowns (legacy name from metrics datasources)
  async metricFindQuery(query: SQLQuery | string, options?: any): Promise<MetricFindValue[]> {
      // Migrate query to v2 with variable context (handles string, legacy formats, etc.)
      const migratedQuery = migrateQuery(query, QueryContext.Variable);

      const target: SQLQuery = {
        ...migratedQuery,
        refId: 'metricFindQuery',
        rawEditor: true,
        format: QueryFormat.Table,
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
    // Migrate query before processing
    const migratedQuery = migrateQuery(query, QueryContext.Panel);

    const interpolatedQuery: SQLQuery = {
      ...migratedQuery,
      query: getTemplateSrv().replace(migratedQuery.query, scopedVars, this.interpolateVariable),
      queryText: migratedQuery.queryText ? getTemplateSrv().replace(migratedQuery.queryText, scopedVars, this.interpolateVariable) : undefined,
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
