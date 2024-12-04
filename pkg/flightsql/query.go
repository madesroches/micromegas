package flightsql

import (
	"github.com/grafana/grafana-plugin-sdk-go/data/sqlutil"
)

type Query struct {
	SQL    string
	Format sqlutil.FormatQueryOption
	RefID         string
}
