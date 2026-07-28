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
  transform: {
    ...require('./.config/jest.config').transform,
    '^.+\\.mjs$': require('./.config/jest.config').transform['^.+\\.(t|j)sx?$'],
  },
};
