package main

import (
	"os"

	"github.com/grafana/grafana-plugin-sdk-go/backend/datasource"
	"github.com/grafana/grafana-plugin-sdk-go/backend/log"
	"github.com/madesroches/grafana-micromegas-datasource/pkg/flightsql"
)

func main() {
	if err := datasource.Manage("micromegas-micromegas-datasource", flightsql.NewDatasource, datasource.ManageOpts{}); err != nil {
		log.DefaultLogger.Error(err.Error())
		os.Exit(1)
	}
}
