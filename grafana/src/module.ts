import {DataSourcePlugin} from '@grafana/data'
import {FlightSQLDataSource} from './datasource'
import {ConfigEditor} from './components/ConfigEditor'
import {QueryEditor} from './components/QueryEditor'
import {SQLQuery, FlightSQLDataSourceOptions, SecureJsonData} from './types'

export const plugin = new DataSourcePlugin<FlightSQLDataSource, SQLQuery, FlightSQLDataSourceOptions, SecureJsonData>(
  FlightSQLDataSource
)
  .setConfigEditor(ConfigEditor)
  .setQueryEditor(QueryEditor)
