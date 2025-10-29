package flightsql

import (
	"github.com/grafana/grafana-plugin-sdk-go/data/sqlutil"
	"google.golang.org/grpc/metadata"
)

type Query struct {
	SQL      string
	Format   sqlutil.FormatQueryOption
	RefID    string
	Metadata metadata.MD
}
