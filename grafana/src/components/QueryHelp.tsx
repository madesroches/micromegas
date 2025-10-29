import React from 'react'

import {Alert} from '@grafana/ui'

export const QueryHelp = () => (
  <Alert title="Query Help" severity="info">
    <div>
      <h3>Format Options:</h3>
      <h4>Table(default):</h4>
      <p>Return any set of columns</p>
      <h3>Time series:</h3>
      <p>Return column named time (UTC in seconds or timestamp) return column(s) with numeric datatype as values</p>
      <p>Result sets of time series queries need to be sorted by time.</p>
    </div>
    <div>
      <h3>Supported Macros:</h3>
      <li>
        $__dateBin(time) -&gt; date_bin(interval &apos;30 second&apos;, time, timestamp
        &apos;1970-01-01T00:00:00Z&apos;)
      </li>
      <li>
        $__dateBinAlias(time) -&gt; date_bin(interval &apos;30 second&apos;, time, timestamp
        &apos;1970-01-01T00:00:00Z&apos;) as time_binned
      </li>
      <li>$__interval -&gt; interval &apos;30 second&apos;</li>
      <li>
        $__timeFilter() -&gt; time &gt;= &apos;2023-01-26T16:24:39Z&apos; AND time &lt;=
        &apos;2023-01-26T17:24:39Z&apos;
      </li>
      <li>$__timeFrom -&gt; cast(&apos;2023-01-01T00:00:00Z&apos; as timestamp)</li>
      <li>
        $__timeGroup(time, hour) -&gt; datepart(&apos;minute&apos;, time),datepart(&apos;hour&apos;,
        time),datepart(&apos;day&apos;, time),datepart(&apos;month&apos;, time),datepart(&apos;year&apos;, time);
      </li>
      <li>
        $__timeGroupAlias(time, minute) -&gt; datepart(&apos;minute&apos;, time) as
        time_minute,datepart(&apos;hour&apos;, time) as time_hour,datepart(&apos;day&apos;, time) as
        time_day,datepart(&apos;month&apos;, time) as time_month, datepart(&apos;year&apos;, time) as time_year
      </li>
      <li>
        $__timeRange -&gt; time &gt;= &apos;2023-01 01T00:00:00Z&apos; and time &lt;=
        &apos;2023-01-01T01:00:00Z&apos;
      </li>
      <li>$__timeRangeFrom(time) -&gt; time &gt;= &apos;2023-01-01T00:00:00Z&apos;</li>
      <li> $__timeRangeTo(time) -&gt; time &lt;= &apos;2023-01-01T01:00:00Z&apos;</li>
      <li>$__timeTo(time) -&gt; cast(time as timestamp)</li>
    </div>
  </Alert>
)
