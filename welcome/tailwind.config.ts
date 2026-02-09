import type { Config } from 'tailwindcss'

const config: Config = {
  content: [
    './index.html',
    './src/**/*.{js,ts,jsx,tsx}',
  ],
  theme: {
    extend: {
      colors: {
        app: {
          bg: 'var(--app-bg)',
          panel: 'var(--panel-bg)',
          card: 'var(--card-bg)',
        },
        brand: {
          rust: 'var(--brand-rust)',
          'rust-dark': 'var(--brand-rust-dark)',
          blue: 'var(--brand-blue)',
          'blue-dark': 'var(--brand-blue-dark)',
          gold: 'var(--brand-gold)',
          'gold-dark': 'var(--brand-gold-dark)',
        },
        theme: {
          border: {
            DEFAULT: 'var(--border-color)',
            hover: 'var(--border-hover)',
          },
          text: {
            primary: 'var(--text-primary)',
            secondary: 'var(--text-secondary)',
            muted: 'var(--text-muted)',
          },
        },
      },
    },
  },
  plugins: [],
}
export default config
