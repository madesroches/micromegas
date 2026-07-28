// force timezone to UTC to allow tests to work regardless of local timezone
// generally used by snapshots, but can affect specific tests
process.env.TZ = 'UTC';

module.exports = {
  // Jest configuration provided by Grafana scaffolding
  ...require('./.config/jest.config'),
    // Inform jest to only transform specific node_module packages.
    transformIgnorePatterns: ["node_modules/?!(d3-interpolate)"],
  // cookie-es (pulled in transitively via react-router) ships only a .mjs build; the scaffolded
  // transform regex only matches .ts/.tsx/.js/.jsx, so .mjs files are never transformed otherwise.
  // Reuse whichever transformer the scaffolded config defines (rather than indexing by its literal
  // key string) so a future regeneration of .config/jest.config.js with a differently-worded key
  // doesn't silently break this lookup.
  transform: {
    ...require('./.config/jest.config').transform,
    '^.+\\.mjs$': Object.values(require('./.config/jest.config').transform)[0],
  },
};
