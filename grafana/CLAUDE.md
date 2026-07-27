# Grafana Plugin (grafana/)

## Essential Commands (from `grafana/` directory)
- **IMPORTANT**: Use `yarn`, NOT `npm` (project uses Yarn 4 / Berry via corepack — run `corepack enable` once on a new machine)
- **Install**: `yarn install`
- **Build**: `yarn build`
- **Dev build**: `yarn dev`
- **Lint**: `yarn lint:fix` (REQUIRED before commit)
- **Test server**: `yarn server` (starts local Grafana with plugin)

## Dependency notes
- `@openfeature/web-sdk` and `@openfeature/core` in `package.json` are not imported anywhere in `grafana/src` — do not remove them as "unused". `@grafana/runtime`'s bundle eagerly `require()`s `@openfeature/ofrep-web-provider`, which transitively needs both at module-load time; removing `@openfeature/web-sdk` breaks Jest and the webpack build (`Cannot find module`), and `@openfeature/core` is there only to satisfy `@openfeature/web-sdk`'s own peer dependency and silence a `yarn install` warning. Verify with `yarn test:ci`/`yarn build`, not just a source-code grep, before touching either.
