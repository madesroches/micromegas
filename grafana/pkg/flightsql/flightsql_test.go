package flightsql

import (
	"context"
	"encoding/json"
	"testing"

	"github.com/apache/arrow/go/v12/arrow/flight"
	"github.com/apache/arrow/go/v12/arrow/flight/flightsql"
	"github.com/apache/arrow/go/v12/arrow/flight/flightsql/example"
	"github.com/apache/arrow/go/v12/arrow/memory"
	"github.com/grafana/grafana-plugin-sdk-go/backend"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestIntegration_QueryData(t *testing.T) {
	db, err := example.CreateDB()
	require.NoError(t, err)
	defer db.Close()

	sqliteServer, err := example.NewSQLiteFlightSQLServer(db)
	require.NoError(t, err)
	sqliteServer.Alloc = memory.NewCheckedAllocator(memory.DefaultAllocator)
	server := flight.NewServerWithMiddleware(nil)
	server.RegisterFlightService(flightsql.NewFlightServer(sqliteServer))
	err = server.Init("localhost:0")
	require.NoError(t, err)
	go server.Serve()
	defer server.Shutdown()

	cfg := config{
		Addr:   server.Addr().String(),
		Token:  "secret",
		Secure: false,
	}
	cfgJSON, err := json.Marshal(cfg)
	require.NoError(t, err)

	settings := backend.DataSourceInstanceSettings{JSONData: cfgJSON}
	ds, err := NewDatasource(context.Background(), settings)
	require.NoError(t, err)

	resp, err := ds.(*FlightSQLDatasource).QueryData(context.Background(),
		&backend.QueryDataRequest{
			Queries: []backend.DataQuery{
				{
					RefID: "A",
					JSON:  mustQueryJSON(t, "A", "select * from intTable"),
				},
				{
					RefID: "B",
					JSON:  mustQueryJSON(t, "B", "select 1"),
				},
			},
		},
	)
	require.NoError(t, err)
	require.Len(t, resp.Responses, 2)

	respA := resp.Responses["A"]
	require.NoError(t, respA.Error)
	frame := respA.Frames[0]

	require.Equal(t, "id", frame.Fields[0].Name)
	require.Equal(t, "keyName", frame.Fields[1].Name)
	require.Equal(t, "value", frame.Fields[2].Name)
	require.Equal(t, "foreignId", frame.Fields[3].Name)
	for _, f := range frame.Fields {
		assert.Equal(t, 4, f.Len())
	}
}

func mustQueryJSON(t *testing.T, refID, sql string) []byte {
	t.Helper()

	b, err := json.Marshal(queryRequest{
		RefID:  refID,
		Text:   sql,
		Format: "table",
	})
	if err != nil {
		panic(err)
	}
	return b
}
