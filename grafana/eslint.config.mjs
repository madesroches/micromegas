import grafana from '@grafana/eslint-config';

export default [
  ...(Array.isArray(grafana) ? grafana : [grafana]),
  {
    rules: {
      'react/prop-types': 'off',
    },
  },
];
