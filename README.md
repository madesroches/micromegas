:warning: Recently forked from Influx's FlightSQL grafana plugin. The intent is to make a grafana datasource plugin for [Micromegas](https://github.com/madesroches/micromegas) while keeping the protocol compatible with FlightSQL.

# Grafana Micromegas Datasource

## Usage

### Adding a Flight SQL Datasource

1. Open the side menu by clicking the Grafana icon in the top header.
1. In the side menu under the Dashboards link you should find a link named Data Sources.
1. Click the + Add data source button in the top header.
1. Select FlightSQL from the Type dropdown.

### Configuring the Plugin

- **Host:** Provide the host:port of your Flight SQL client.
- **AuthType** Select between none, username/password and token.
- **Token:** If auth type is token provide a bearer token for accessing your client.
- **Username/Password** iF auth type is username and password provide a username and password.
- **Require TLS/SSL:** Either enable or disable TLS based on the configuration of your client.

- **MetaData** Provide optional key, value pairs that you need sent to your Flight SQL client.

### Using the Query Builder

The default view is a query builder which is in active development:

- Begin by selecting the table from the dropdown.
- This will auto populate your available columns for your select statement. Use the **+** and **-** buttons to add or remove additional where statements.
- You can overwrite a dropdown field by typing in your desired value (e.g. `*`).
- The where field is a text entry where you can define any where clauses. Use the + and - buttons to add or remove additional where statements.
- You can switch to a raw SQL input by pressing the "Edit SQL" button. This will show you the query you have been building thus far and allow you to enter any query.
- Press the "Run query" button to see your results.
- From there you can add to dashboards and create any additional dashboards you like.

## Development

See [DEVELOPMENT.md](DEVELOPMENT.md).
