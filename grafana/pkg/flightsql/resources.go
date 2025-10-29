package flightsql

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"sort"
	"time"

	"github.com/apache/arrow/go/v12/arrow/array"
	"github.com/apache/arrow/go/v12/arrow/flight"
	"github.com/apache/arrow/go/v12/arrow/flight/flightsql"
	"github.com/apache/arrow/go/v12/arrow/memory"
	"github.com/grafana/grafana-plugin-sdk-go/backend"
	"github.com/grafana/grafana-plugin-sdk-go/data/sqlutil"
	"google.golang.org/grpc/metadata"
)

func (d *FlightSQLDatasource) getMacros(w http.ResponseWriter, r *http.Request) {
	size := len(sqlutil.DefaultMacros) + len(macros)
	names := make([]string, 0, size)
	for k := range sqlutil.DefaultMacros {
		if k == "table" || k == "column" {
			// We don't have the information available for these to function
			// propperly so omit them from advertisement.
			continue
		}
		names = append(names, k)
	}
	for k := range macros {
		names = append(names, k)
	}
	sort.Strings(names)

	err := json.NewEncoder(w).Encode(struct {
		Macros []string `json:"macros"`
	}{
		Macros: names,
	})
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
}

func (d *FlightSQLDatasource) getSQLInfo(w http.ResponseWriter, r *http.Request) {
	ctx, cancel := context.WithTimeout(r.Context(), 30*time.Second)
	defer cancel()
	ctx = metadata.NewOutgoingContext(ctx, d.md)
	info, err := d.client.GetSqlInfo(ctx, []flightsql.SqlInfo{})
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	reader, err := d.client.DoGet(ctx, info.Endpoint[0].Ticket)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	defer reader.Release()

	if err := writeDataResponse(w, newDataResponse(reader)); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
}

func (d *FlightSQLDatasource) getTables(w http.ResponseWriter, r *http.Request) {
	ctx, cancel := context.WithTimeout(r.Context(), 30*time.Second)
	defer cancel()
	ctx = metadata.NewOutgoingContext(ctx, d.md)

	info, err := d.client.GetTables(ctx, &flightsql.GetTablesOpts{
		TableTypes: []string{"BASE TABLE", "table"},
	})
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	reader, err := d.client.DoGet(ctx, info.Endpoint[0].Ticket)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	defer reader.Release()

	if err := writeDataResponse(w, newDataResponse(reader)); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
}

func (d *FlightSQLDatasource) getColumns(w http.ResponseWriter, r *http.Request) {
	tableName := r.URL.Query().Get("table")
	if tableName == "" {
		http.Error(w, `query parameter "table" is required`, http.StatusBadRequest)
		return
	}

	ctx, cancel := context.WithTimeout(r.Context(), 30*time.Second)
	defer cancel()
	ctx = metadata.NewOutgoingContext(ctx, d.md)
	info, err := d.client.GetTables(ctx, &flightsql.GetTablesOpts{
		TableNameFilterPattern: &tableName,
		IncludeSchema:          true,
	})
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	reader, err := d.client.DoGet(ctx, info.Endpoint[0].Ticket)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	defer reader.Release()

	if !reader.Next() {
		http.Error(w, "table not found", http.StatusNotFound)
		return
	}
	rec := reader.Record()
	rec.Retain()
	defer rec.Release()
	reader.Next()
	if err := reader.Err(); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	indices := rec.Schema().FieldIndices("table_schema")
	if len(indices) == 0 {
		http.Error(w, "table_schema field not found", http.StatusInternalServerError)
		return
	}
	col := rec.Column(indices[0])
	serializedSchema := array.NewStringData(col.Data()).Value(0)
	schema, err := flight.DeserializeSchema([]byte(serializedSchema), memory.DefaultAllocator)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	var resp backend.DataResponse
	resp.Frames = append(resp.Frames, newFrame(schema))
	if err := writeDataResponse(w, resp); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
}

func newDataResponse(reader recordReader) backend.DataResponse {
	var resp backend.DataResponse
	frame := newFrame(reader.Schema())
READER:
	for reader.Next() {
		record := reader.Record()
		for i, col := range record.Columns() {
			if err := copyData(frame.Fields[i], col); err != nil {
				resp.Error = err
				break READER
			}
		}
		if err := reader.Err(); err != nil && !errors.Is(err, io.EOF) {
			resp.Error = err
			break
		}
	}
	resp.Frames = append(resp.Frames, frame)
	return resp
}

func writeDataResponse(w io.Writer, resp backend.DataResponse) error {
	json, err := resp.MarshalJSON()
	if err != nil {
		return err
	}
	_, err = w.Write(json)
	return err
}
