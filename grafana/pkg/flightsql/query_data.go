package flightsql

import (
	"context"
	"encoding/json"
	"fmt"
	"runtime/debug"
	"strconv"
	"sync"
	"time"

	"github.com/grafana/grafana-plugin-sdk-go/backend"
	"github.com/grafana/grafana-plugin-sdk-go/data/sqlutil"
	"google.golang.org/grpc/metadata"
)

// QueryData executes batches of ad-hoc queries and returns a batch of results.
func (d *FlightSQLDatasource) QueryData(ctx context.Context, req *backend.QueryDataRequest) (*backend.QueryDataResponse, error) {
	// Create request-scoped metadata to avoid mutating shared d.md
	// This prevents race conditions and user data leakage between concurrent requests
	requestMd := metadata.MD{}

	// Extract user information from plugin context and pass to FlightSQL server
	// Uses generic header names that work for any client (Grafana, Python services, etc.)
	// Only send if user attribution is enabled (default: true, can be disabled for privacy)
	if d.enableUserAttribution && req.PluginContext.User != nil {
		user := req.PluginContext.User

		// Add end-user identity to gRPC metadata
		// FlightSQL server can log these headers for attribution
		if user.Login != "" {
			requestMd.Set("x-user-id", user.Login) // Generic: works for any client
		}
		if user.Email != "" {
			requestMd.Set("x-user-email", user.Email) // Generic: works for any client
		}
		if user.Name != "" {
			requestMd.Set("x-user-name", user.Name) // Generic: works for any client
		}

		// Add organization/tenant context
		if req.PluginContext.OrgID > 0 {
			requestMd.Set("x-org-id", fmt.Sprintf("%d", req.PluginContext.OrgID)) // Generic: tenant ID
		}

		// Indicate the client type (useful when multiple client types exist)
		requestMd.Set("x-client-type", "grafana")

		logInfof("Query from user: %s (%s) via Grafana", user.Login, user.Email)
	}

	// Refresh OAuth token if using OAuth authentication
	if d.oauthMgr != nil {
		token, err := d.oauthMgr.GetToken(ctx)
		if err != nil {
			logErrorf("OAuth token refresh failed: %v", err)
			// Return error for all queries
			response := backend.NewQueryDataResponse()
			for _, q := range req.Queries {
				response.Responses[q.RefID] = backend.DataResponse{
					Error: fmt.Errorf("OAuth token refresh failed: %v", err),
				}
			}
			return response, nil
		}

		// Add fresh token to request-scoped metadata
		requestMd.Set("Authorization", fmt.Sprintf("Bearer %s", token))
	}

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

		// Join request-scoped metadata with query-specific metadata
		query.Metadata = metadata.Join(requestMd, query.Metadata)

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

	query_metadata := make(map[string]string)
	if q.TimeFilter {
		query_metadata["query_range_begin"] = dataQuery.TimeRange.From.Format(time.RFC3339Nano)
		query_metadata["query_range_end"] = dataQuery.TimeRange.To.Format(time.RFC3339Nano)
	}
	if q.AutoLimit {
		query_metadata["limit"] = strconv.FormatInt(q.MaxDataPoints, 10)
	}
	var fsqlQuery = &Query{
		SQL:      sql,
		Format:   format,
		RefID:    q.RefID,
		Metadata: metadata.New(query_metadata),
	}
	return fsqlQuery, nil
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
	AutoLimit            bool   `json:"autoLimit"`
}

// query executes a SQL statement by issuing a `CommandStatementQuery` command to Flight SQL.
func (d *FlightSQLDatasource) query(ctx context.Context, query Query) (resp backend.DataResponse) {
	defer func() {
		if r := recover(); r != nil {
			logErrorf("Panic: %s %s", r, string(debug.Stack()))
			resp = backend.ErrDataResponse(backend.StatusInternal, fmt.Sprintf("panic: %s", r))
		}
	}()

	var md = metadata.Join(d.md, query.Metadata)
	if md.Len() != 0 {
		ctx = metadata.NewOutgoingContext(ctx, md)
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
