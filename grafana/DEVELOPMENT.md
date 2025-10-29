# Developing the Plugin

These instructions assume you have the following tools installed:

- [`yarn`](https://yarnpkg.com)
- [`mage`](https://magefile.org)

## Frontend

1. Install dependencies:

   ```shell
   $ yarn install
   ```

1. Build plugin in development mode:

   ```shell
   $ yarn dev
   ```

1. Run the linter prior to merging commits:

   ```shell
   $ yarn lint:fix
   ```

## Backend

1. Build backend plugin binaries for Linux, Windows and Darwin:

   ```shell
   $ mage -v
   ```

   :warning: Mage has been configured **not** build the `linux-arm` target, because the
   Apache Arrow project doesn't provide it in its build matrix.

1. List all available Mage targets for additional commands:

   ```shell
   $ mage -l
   ```

1. Run the plugin using Docker Compose:

   ```shell
   $ yarn server
   ```

1. Navigate to the [Locally Running Grafana](http://localhost:3000/).
1. Follow the instructions in [Adding a Flight SQL
   Datasource](/#adding-a-flight-sql-datasource).
