import grafanaConfig from '@grafana/eslint-config/flat.js';

export default [
  {
    ignores: [
      'dist/',
      'coverage/',
      'node_modules/',
      '*.config.js',
      '*.config.ts',
      'micromegas-micromegas-datasource/',
      'old-plugin-v*/',
    ],
  },
  ...grafanaConfig,
  {
    rules: {
      'react/prop-types': 'off',
    },
  },
];
