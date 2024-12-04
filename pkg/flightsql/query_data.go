package flightsql

import (
	"context"
	"encoding/json"
	"fmt"
	"runtime/debug"
	"sync"
	"time"

	"github.com/grafana/grafana-plugin-sdk-go/backend"
	"github.com/grafana/grafana-plugin-sdk-go/data/sqlutil"
	"google.golang.org/grpc/metadata"
)

// QueryData executes batches of ad-hoc queries and returns a batch of results.
func (d *FlightSQLDatasource) QueryData(ctx context.Context, req *backend.QueryDataRequest) (*backend.QueryDataResponse, error) {
	var (
		wg             sync.WaitGroup
		response       = backend.NewQueryDataResponse()
		executeResults = make(chan executeResult, len(req.Queries))
	)

	for _, dataQuery := range req.Queries {
		query, err := decodeQueryRequest(dataQuery)
		if err != nil {
			response.Responses[dataQuery.RefID] = backend.ErrDataResponse(backend.StatusBadRequest, err.Error())
			continue
		}

		wg.Add(1)
		go func() {
			defer wg.Done()
			executeResults <- executeResult{
				refID:        query.RefID,
				dataResponse: d.query(ctx, *query),
			}
		}()
	}

	wg.Wait()
	close(executeResults)
	for r := range executeResults {
		response.Responses[r.refID] = r.dataResponse
	}

	return response, nil
}

// decodeQueryRequest decodes a [backend.DataQuery] and returns a
// [*sqlutil.Query] where all macros are expanded.
func decodeQueryRequest(dataQuery backend.DataQuery) (*Query, error) {
	var q queryRequest
	if err := json.Unmarshal(dataQuery.JSON, &q); err != nil {
		return nil, fmt.Errorf("unmarshal json: %w", err)
	}

	logInfof("q=%+v", q)

	var format sqlutil.FormatQueryOption
	switch q.Format {
	case "time_series":
		format = sqlutil.FormatOptionTimeSeries
	case "table":
		format = sqlutil.FormatOptionTable
	case "logs":
		format = sqlutil.FormatOptionLogs
	default:
		format = sqlutil.FormatOptionTimeSeries
	}

	query := &sqlutil.Query{
		RawSQL:        q.Text,
		RefID:         q.RefID,
		MaxDataPoints: q.MaxDataPoints,
		Interval:      time.Duration(q.IntervalMilliseconds) * time.Millisecond,
		TimeRange:     dataQuery.TimeRange,
		Format:        format,
	}

	// Process macros and execute the query.
	sql, err := sqlutil.Interpolate(query, macros)
	if err != nil {
		return nil, fmt.Errorf("macro interpolation: %w", err)
	}
	query.RawSQL = sql

	return &Query{
		SQL:    sql,
		Format: format,
		RefID:         q.RefID,
	}, nil
}

// executeResult is an envelope for concurrent query responses.
type executeResult struct {
	refID        string
	dataResponse backend.DataResponse
}

// queryRequest is an inbound query request as part of a batch of queries sent
// to [(*FlightSQLDatasource).QueryData].
type queryRequest struct {
	RefID                string `json:"refId"`
	Text                 string `json:"queryText"`
	IntervalMilliseconds int    `json:"intervalMs"`
	MaxDataPoints        int64  `json:"maxDataPoints"`
	Format               string `json:"format"`
	TimeFilter           bool   `json:"timeFilter"`
}

// query executes a SQL statement by issuing a `CommandStatementQuery` command to Flight SQL.
func (d *FlightSQLDatasource) query(ctx context.Context, query Query) (resp backend.DataResponse) {
	defer func() {
		if r := recover(); r != nil {
			logErrorf("Panic: %s %s", r, string(debug.Stack()))
			resp = backend.ErrDataResponse(backend.StatusInternal, fmt.Sprintf("panic: %s", r))
		}
	}()

	if d.md.Len() != 0 {
		ctx = metadata.NewOutgoingContext(ctx, d.md)
	}

	info, err := d.client.Execute(ctx, query.SQL)
	if err != nil {
		return backend.ErrDataResponse(backend.StatusInternal, fmt.Sprintf("flightsql: %s", err))
	}
	if len(info.Endpoint) != 1 {
		return backend.ErrDataResponse(backend.StatusInternal, fmt.Sprintf("unsupported endpoint count in response: %d", len(info.Endpoint)))
	}
	reader, err := d.client.DoGetWithHeaderExtraction(ctx, info.Endpoint[0].Ticket)
	if err != nil {
		return backend.ErrDataResponse(backend.StatusInternal, fmt.Sprintf("flightsql: %s", err))
	}
	defer reader.Release()

	headers, err := reader.Header()
	if err != nil {
		logErrorf("Failed to extract headers: %s", err)
	}

	return newQueryDataResponse(reader, query, headers)
}
